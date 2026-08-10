// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use std::{env, ffi::OsStr, path::PathBuf};

use chrono::Local;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use solstone_core_cli::{
    BodyAppleOptions, BodyCommand, BodyOuraCommand, BodyOuraConnectOptions, BodyOuraSyncOptions,
    BodyRebuildOptions, BrainCommand, BrainInspectOptions, BrainPrerequisiteRenewalSessionOptions,
    BrainRefreshExpectArg, BrainRefreshSessionOptions, BrainRuntimeFailureOptions, CogitateCommand,
    Command, ConveyOptions, GenerateCommand, GenerateSessionOptions, IndexerCommand,
    IndexerCountsOptions, IndexerFoldEntityEdgesOptions, IndexerOptions, IndexerPrunePathsOptions,
    IndexerPruneStreamOptions, IndexerQueryOptions, IndexerReadOptions, IndexerSearchOptions,
    InstallCommand, JournalConfigCommand, JournalConfigCommitOptions, JournalConfigExpectArg,
    JournalConfigReadOptions, JournalPathOptions, LocalCommand, ServiceOptions,
    SpeakerResolveCommand, SplCommand, TransferCommand, TransferExportOptions,
    TransferImportOptions, USAGE, evaluate_args, version_line,
};
mod supervisor;
use solstone_core_indexer_query::{
    IndexAccessError, Order, SearchRequest, agents, coverage, search, search_counts,
};
use solstone_core_indexer_store::db::{prune_by_paths, prune_chunks_by_stream, reset_index};
use solstone_core_indexer_store::merge::{
    fingerprint_edge_rows, fold_entity_edges_for_recorded_merge,
    rebuild_edges_for_recorded_merge_undo,
};
use solstone_core_indexer_store::scan::{
    RescanFileStatus, rebuild_edges, rescan_file, scan_journal,
};
use solstone_core_journal::{
    ConfigError, HomeError, Source, discover_home, ensure_journal_dir_with_label,
    read_config_journal, resolve_journal_path,
};
use solstone_core_journal_config::{materialized_defaults, read_journal_config};
use solstone_core_journal_config_write::{
    CommitConfigError, ConfigExpectation, LockError, LockOptions, commit_journal_config,
};
use solstone_core_observer::{CREATE_RETIRED_MESSAGE, ObserverCommand};
const EXIT_USAGE: u8 = 64;
const EXIT_UNAVAILABLE: u8 = 69;
const EXIT_INTERNAL_FAILURE: u8 = 70;
const EXIT_TEMPFAIL: u8 = 75;
const EXIT_DATAERR: u8 = 65;
const EXIT_CANTCREAT: u8 = 73;
const EXIT_IOERR: u8 = 74;
/// `EX_PROTOCOL`: the caller broke a brain-session framing contract.
const EXIT_PROTOCOL: u8 = 76;
const EXIT_PROVIDER_CONFIGURATION: u8 = 78;
const MAX_JSON_STDIN_BYTES: usize = 1024 * 1024;
const MAX_LOCAL_STDIN_BYTES: usize = 1024 * 1024;
const SESSION_INPUT_TIMEOUT: Duration = Duration::from_secs(90);
const SESSION_RESULT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const REFRESH_PROBE_SCHEMA: &str = "solstone.brain.refresh.probe.v1";
const REFRESH_ABANDON_SCHEMA: &str = "solstone.brain.refresh.abandon.v1";
const REFRESH_TERMINAL_SCHEMA: &str = "solstone.brain.refresh.terminal.v1";
const REFRESH_RESULT_SCHEMA: &str = "solstone.brain.refresh.result.v1";
const REFRESH_READY_SCHEMA: &str = "solstone.brain.refresh.ready.v1";
const PREREQUISITE_RENEWAL_PROBE_SCHEMA: &str = "solstone.brain.prerequisite_renewal.probe.v1";
const PREREQUISITE_RENEWAL_ABANDON_SCHEMA: &str = "solstone.brain.prerequisite_renewal.abandon.v1";
const PREREQUISITE_RENEWAL_TERMINAL_SCHEMA: &str =
    "solstone.brain.prerequisite_renewal.terminal.v1";
const PREREQUISITE_RENEWAL_RESULT_SCHEMA: &str = "solstone.brain.prerequisite_renewal.result.v1";
const PREREQUISITE_RENEWAL_READY_SCHEMA: &str = "solstone.brain.prerequisite_renewal.ready.v1";
const MAX_LOCAL_GENERATE_STDIN_BYTES: usize = 64 * 1024 * 1024;
const ZERO_EDGE_HINT: &str = "Zero edges indexed: edges are talent-derived, and the --rescan-full edge phase remains modification-time incremental — run journal indexer --rebuild-edges to force full edge re-extraction.";
struct JournalPathLine {
    label: &'static str,
    path: PathBuf,
}

enum JournalPathError {
    Config(ConfigError),
    Home(HomeError),
    Create(solstone_core_journal::EnsureJournalDirError),
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match evaluate_args(&args) {
        Ok(Command::Version) => {
            print!("{}", version_line(env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Ok(Command::JournalPath(options)) => match run_journal_path(options) {
            Ok(line) => {
                println!("{}\t{}", line.label, line.path.display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprint_journal_path_error(error);
                ExitCode::from(EXIT_TEMPFAIL)
            }
        },
        Ok(Command::Indexer(command)) => run_indexer(*command),
        Ok(Command::JournalConfig(command)) => run_journal_config(command),
        Ok(Command::SpeakerTranscriptWrite) => run_speaker_transcript_write(),
        Ok(Command::SpeakerResolve(command)) => run_speaker_resolve(command),
        Ok(Command::Local(command)) => run_local(command),
        Ok(Command::Generate(command)) => run_generate(command),
        Ok(Command::Cogitate(command)) => run_cogitate(command),
        Ok(Command::Brain(command)) => run_brain(command),
        Ok(Command::Body(command)) => run_body(command),
        Ok(Command::Transfer(command)) => run_transfer(command),
        Ok(Command::Convey(options)) => run_convey(options),
        Ok(Command::Spl(command)) => run_spl_process(command),
        Ok(Command::Supervisor(options)) => supervisor::run(options),
        Ok(Command::Observer(command)) => run_observer(command),
        Err(_) => {
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn run_transfer(command: TransferCommand) -> ExitCode {
    match command {
        TransferCommand::Export(options) => run_transfer_export(options),
        TransferCommand::Import(options) => run_transfer_import(options),
    }
}

fn run_transfer_export(options: TransferExportOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match solstone_core_transfer::export(
        &journal,
        solstone_core_transfer::ExportRequest {
            day: options.day,
            output: PathBuf::from(options.output),
        },
    ) {
        Ok(report) => {
            println!(
                "exported day={} segments={} files={} output={}",
                report.day,
                report.segments,
                report.files,
                report.output.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("transfer export failed: {error}");
            ExitCode::from(match error {
                solstone_core_transfer::TransferError::MissingDay(_)
                | solstone_core_transfer::TransferError::NoSegments(_) => 2,
                solstone_core_transfer::TransferError::InvalidDay
                | solstone_core_transfer::TransferError::Manifest(_) => EXIT_DATAERR,
                _ => EXIT_IOERR,
            })
        }
    }
}

fn run_transfer_import(options: TransferImportOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match solstone_core_transfer::import(
        &journal,
        solstone_core_transfer::ImportRequest {
            archive: PathBuf::from(options.archive),
            dry_run: options.dry_run,
        },
    ) {
        Ok(report) => {
            print_transfer_import_report(&report);
            ExitCode::SUCCESS
        }
        Err(solstone_core_transfer::ImportError::Partial { report, reason }) => {
            print_transfer_import_report(&report);
            eprintln!("transfer import failed: {reason}");
            ExitCode::from(EXIT_IOERR)
        }
        Err(solstone_core_transfer::ImportError::Fatal(error)) => {
            eprintln!("transfer import failed: {error}");
            ExitCode::from(match error {
                solstone_core_transfer::TransferError::InvalidDay
                | solstone_core_transfer::TransferError::Manifest(_)
                | solstone_core_transfer::TransferError::ArchiveMember(_)
                | solstone_core_transfer::TransferError::ContentMismatch(_) => EXIT_DATAERR,
                _ => EXIT_IOERR,
            })
        }
    }
}

fn print_transfer_import_report(report: &solstone_core_transfer::ImportReport) {
    for outcome in &report.outcomes {
        match outcome {
            solstone_core_transfer::SegmentOutcome::Landed { source, target } => {
                println!("{source}: landed={target}");
            }
            solstone_core_transfer::SegmentOutcome::LandedDeconflicted { source, target } => {
                println!("{source}: landed-deconflicted={target}");
            }
            solstone_core_transfer::SegmentOutcome::SkippedAlreadySynced { source } => {
                println!("{source}: skipped-already-synced");
            }
            solstone_core_transfer::SegmentOutcome::Failed { source, reason } => {
                println!("{source}: failed={reason}");
            }
            solstone_core_transfer::SegmentOutcome::NotAttempted { source } => {
                println!("{source}: not-attempted");
            }
        }
    }
    println!(
        "day={} dry-run={} landed={} skipped={} deconflicted={} failed={} not-attempted={} rescan={}",
        report.day,
        report.dry_run,
        report.landed(),
        report.skipped(),
        report.deconflicted(),
        report.failed(),
        report.not_attempted(),
        report.rescan.as_str(),
    );
}

fn run_convey(options: ConveyOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    match solstone_core_convey_shell::run_convey(journal, options.port) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
    }
}

fn run_observer(command: ObserverCommand) -> ExitCode {
    if matches!(command, ObserverCommand::Create) {
        eprint!("{CREATE_RETIRED_MESSAGE}");
        return ExitCode::from(2);
    }
    let journal = match resolve_journal_config_path(None) {
        Ok(line) => line.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    match solstone_core_observer::execute(
        &journal,
        command,
        solstone_core_observer::system_now_ms(),
    ) {
        Ok(output) => {
            let mut stdout = io::stdout().lock();
            if stdout.write_all(output.as_bytes()).is_err()
                || stdout.write_all(b"\n").is_err()
                || stdout.flush().is_err()
            {
                eprintln!("observer failed: stdout I/O error");
                ExitCode::from(EXIT_IOERR)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("{error}");
            if error.is_user_error() {
                ExitCode::from(1)
            } else {
                ExitCode::from(EXIT_IOERR)
            }
        }
    }
}

fn run_speaker_resolve(command: SpeakerResolveCommand) -> ExitCode {
    let limit = if matches!(command, SpeakerResolveCommand::AccumulateVoiceprints) {
        MAX_LOCAL_GENERATE_STDIN_BYTES
    } else {
        MAX_JSON_STDIN_BYTES
    };
    let value = match read_bounded_json_value(limit) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("speaker-resolve failed: stdin was not a valid JSON request");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let result = match command {
        SpeakerResolveCommand::AccumulateVoiceprints => parse_accumulation_request(value)
            .and_then(|request| {
                solstone_core_speaker_resolve::voiceprint_accumulation::accumulate_voiceprints(
                    &request,
                )
                .map_err(|error| error.to_string())
            })
            .and_then(|outcome| serde_json::to_value(outcome).map_err(|error| error.to_string())),
        SpeakerResolveCommand::WriteOwnerCentroid => write_owner_centroid_request(value),
        SpeakerResolveCommand::RebuildOwnerCentroid => rebuild_owner_centroid_request(value),
        SpeakerResolveCommand::WriteOwnerCandidate => write_owner_candidate_request(value),
        SpeakerResolveCommand::ReadOwnerCandidate => read_owner_candidate_request(value),
        SpeakerResolveCommand::ClearOwnerCandidate => clear_owner_candidate_request(value),
        SpeakerResolveCommand::WriteVoiceprint => write_voiceprint_request(value),
        SpeakerResolveCommand::RemoveVoiceprint => remove_voiceprint_request(value),
        SpeakerResolveCommand::BackfillVoiceprintLastSeen => {
            backfill_voiceprint_last_seen_request(value)
        }
        SpeakerResolveCommand::WriteStubLabels => write_stub_labels_request(value),
        SpeakerResolveCommand::WriteFullLabels => write_full_labels_request(value),
        SpeakerResolveCommand::PatchLabels => patch_labels_request(value),
        SpeakerResolveCommand::RestoreLabelRows => restore_label_rows_request(value),
        SpeakerResolveCommand::AppendCorrection => append_correction_request(value),
        SpeakerResolveCommand::WipeSpeakerArtifacts => wipe_speaker_artifacts_request(value),
        SpeakerResolveCommand::Identify => identify_request(value),
        SpeakerResolveCommand::UndoIdentify => undo_identify_request(value),
        SpeakerResolveCommand::BootstrapVoiceprints => bootstrap_request(value, false),
        SpeakerResolveCommand::SeedFromImports => bootstrap_request(value, true),
        SpeakerResolveCommand::MergeNames => merge_names_request(value),
        SpeakerResolveCommand::Backfill => backfill_request(value),
        SpeakerResolveCommand::BackfillStatus => backfill_status_request(value),
    };
    match result {
        Ok(output) => {
            if serde_json::to_writer(io::stdout().lock(), &output).is_ok() {
                println!();
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_IOERR)
            }
        }
        Err(error) => {
            let exit = speaker_resolve_error_exit(&error);
            eprintln!(
                "{}",
                json!({
                    "error":"speaker_resolve_failed",
                    "detail":error,
                    "exit_code":exit,
                })
            );
            ExitCode::from(exit)
        }
    }
}

fn speaker_resolve_error_exit(error: &str) -> u8 {
    if error.contains("entity not found") || error.contains("No such file or directory") {
        EXIT_UNAVAILABLE
    } else if error.contains("could not acquire lock") || error.contains("storage is busy") {
        EXIT_TEMPFAIL
    } else {
        EXIT_USAGE
    }
}

fn identify_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::discovery_cache::normalize_reviewed_near_match_ids;
    use solstone_core_speaker_resolve::identify_cluster::{
        IdentifyClusterRequest, identify_cluster,
    };
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-identify-request-v1",
        &[
            "schema",
            "journal_root",
            "cluster_id",
            "name",
            "entity_id",
            "resolve_only",
            "create_new",
            "entity_type",
            "request_id",
            "reviewed_near_match_entity_ids",
            "caller",
            "actor",
            "encoder",
        ],
    )?;
    let object = &request_object;
    let reviewed_near_match_entity_ids =
        match normalize_reviewed_near_match_ids(object.get("reviewed_near_match_entity_ids")) {
            Ok(ids) => ids,
            Err(error) => return Ok(error.invalid_request_response()),
        };
    let request = IdentifyClusterRequest {
        journal_root: PathBuf::from(required_string(object, "journal_root")?),
        cluster_id: required_i64(object, "cluster_id")?,
        name: optional_string(object, "name")?,
        entity_id: optional_string(object, "entity_id")?,
        resolve_only: required_bool(object, "resolve_only")?,
        create_new: required_bool(object, "create_new")?,
        entity_type: required_string(object, "entity_type")?,
        request_id: required_string(object, "request_id")?,
        reviewed_near_match_entity_ids,
        caller: optional_string(object, "caller")?.unwrap_or_default(),
        actor: optional_string(object, "actor")?,
    };
    identify_cluster(&request, &encoder(object)?).map_err(|error| error.to_string())
}

fn undo_identify_request(value: Value) -> Result<Value, String> {
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-undo-identify-request-v1",
        &["schema", "journal_root", "operation_id", "encoder"],
    )?;
    let object = &request_object;
    solstone_core_speaker_resolve::identify_undo::undo_identify_operation(
        &PathBuf::from(required_string(object, "journal_root")?),
        &required_string(object, "operation_id")?,
        &encoder(object)?,
    )
    .map_err(|error| error.to_string())
}

fn bootstrap_request(value: Value, imports: bool) -> Result<Value, String> {
    use solstone_core_speaker_resolve::bootstrap::{
        BootstrapOutcome, BootstrapRequest, SeedFromImportsOutcome, bootstrap_voiceprints,
        seed_from_imports,
    };
    let schema = if imports {
        "solstone-speaker-resolve-seed-from-imports-request-v1"
    } else {
        "solstone-speaker-resolve-bootstrap-voiceprints-request-v1"
    };
    let request_object = request_object(
        value,
        schema,
        &["schema", "journal_root", "encoder", "added_at", "dry_run"],
    )?;
    let object = &request_object;
    let request = BootstrapRequest {
        journal_root: PathBuf::from(required_string(object, "journal_root")?),
        encoder: encoder(object)?,
        added_at: required_i64(object, "added_at")?,
        dry_run: required_bool(object, "dry_run")?,
    };
    if imports {
        let outcome = seed_from_imports(&request).map_err(|error| error.to_string())?;
        Ok(match outcome {
            SeedFromImportsOutcome::NoOwnerCentroid => json!({"status":"no_owner_centroid"}),
            SeedFromImportsOutcome::Completed(stats) => json!({
                "status":"completed", "segments_scanned":stats.segments_scanned,
                "segments_with_speakers":stats.segments_with_speakers,
                "speakers_found":stats.speakers_found, "embeddings_saved":stats.embeddings_saved,
                "embeddings_skipped_owner":stats.embeddings_skipped_owner,
                "embeddings_skipped_duplicate":stats.embeddings_skipped_duplicate,
                "speakers_unmatched":stats.speakers_unmatched, "errors":stats.errors,
            }),
        })
    } else {
        let outcome = bootstrap_voiceprints(&request).map_err(|error| error.to_string())?;
        Ok(match outcome {
            BootstrapOutcome::NoOwnerCentroid => json!({"status":"no_owner_centroid"}),
            BootstrapOutcome::Completed(stats) => json!({
                "status":"completed", "segments_scanned":stats.segments_scanned,
                "single_speaker_segments":stats.single_speaker_segments,
                "speakers_found":stats.speakers_found, "entities_created":stats.entities_created,
                "embeddings_saved":stats.embeddings_saved,
                "embeddings_skipped_owner":stats.embeddings_skipped_owner,
                "embeddings_skipped_duplicate":stats.embeddings_skipped_duplicate,
                "speakers_unmatched":stats.speakers_unmatched, "errors":stats.errors,
            }),
        })
    }
}

fn merge_names_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::bootstrap::{MergeNamesOutcome, merge_names};
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-merge-names-request-v1",
        &["schema", "journal_root", "alias_name", "canonical_name"],
    )?;
    let object = &request_object;
    let outcome = merge_names(
        &PathBuf::from(required_string(object, "journal_root")?),
        &required_string(object, "alias_name")?,
        &required_string(object, "canonical_name")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(match outcome {
        MergeNamesOutcome::Ambiguous {
            field,
            ambiguity_id,
            candidates,
        } => json!({
            "status":"ambiguous", "field":field, "ambiguity_id":ambiguity_id,
            "candidates":candidates.into_iter().map(|candidate| json!({
                "id":candidate.id, "name":candidate.name, "tier":candidate.tier as u8,
                "score":candidate.score,
            })).collect::<Vec<_>>(),
        }),
        MergeNamesOutcome::AliasNotFound => json!({"status":"alias_not_found"}),
        MergeNamesOutcome::CanonicalNotFound => json!({"status":"canonical_not_found"}),
        MergeNamesOutcome::SameEntity { entity_id } => {
            json!({"status":"same_entity","entity_id":entity_id})
        }
        MergeNamesOutcome::PrincipalEntity { entity_id } => {
            json!({"status":"principal_entity","entity_id":entity_id})
        }
        MergeNamesOutcome::Ready {
            alias_entity_id,
            canonical_entity_id,
        } => json!({
            "status":"ready", "alias_entity_id":alias_entity_id,
            "canonical_entity_id":canonical_entity_id,
        }),
    })
}

fn backfill_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::backfill::{BackfillRunRequest, run_backfill};
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-backfill-request-v1",
        &[
            "schema",
            "journal_root",
            "operation_id",
            "reattribute",
            "now_ms",
        ],
    )?;
    let object = &request_object;
    let result = run_backfill(&BackfillRunRequest {
        journal_root: PathBuf::from(required_string(object, "journal_root")?),
        operation_id: required_string(object, "operation_id")?,
        reattribute: required_bool(object, "reattribute")?,
        now_ms: required_i64(object, "now_ms")?,
    })
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "operation_id":result.operation_id, "total_count":result.total_count,
        "processed_count":result.processed_count, "skipped_count":result.skipped_count,
        "error_count":result.error_count,
        "error_segments":result.error_segments.into_iter().map(|error| json!({
            "day":error.segment.day, "stream":error.segment.stream,
            "segment_key":error.segment.segment_key, "detail":error.detail,
        })).collect::<Vec<_>>(),
        "pending_count":result.pending_count,
        "done":result.done,
    }))
}

fn backfill_status_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::backfill_operations::{
        backfill_operation_status, backfill_operations_path, load_backfill_operations,
    };
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-backfill-status-request-v1",
        &["schema", "journal_root", "operation_id"],
    )?;
    let object = &request_object;
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let operation_id = required_string(object, "operation_id")?;
    let status = backfill_operation_status(
        &load_backfill_operations(&backfill_operations_path(&root))
            .map_err(|error| error.to_string())?,
        &operation_id,
    )
    .map_err(|error| error.to_string())?;
    Ok(status.map_or_else(
        || json!({"status":"not_found","operation_id":operation_id}),
        |status| {
            json!({
                "status":if status.done { "done" } else { "resumable" },
                "operation_id":operation_id, "total_count":status.total_count,
                "completed_count":status.completed_count, "pending_count":status.pending_count,
                "error_count":status.error_count,
                "error_segments":status.error_segments.into_iter().map(|error| json!({
                    "day":error.segment.day, "stream":error.segment.stream,
                    "segment_key":error.segment.segment_key, "detail":error.detail,
                })).collect::<Vec<_>>(),
                "resumable":status.resumable, "done":status.done,
            })
        },
    ))
}

fn request_object(
    value: Value,
    schema: &str,
    fields: &[&str],
) -> Result<Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    if object.keys().any(|key| !fields.contains(&key.as_str()))
        || object.get("schema").and_then(Value::as_str) != Some(schema)
    {
        return Err(format!("invalid request schema: {schema}"));
    }
    Ok(object.clone())
}

fn encoder(object: &Map<String, Value>) -> Result<solstone_core_entity::EncoderIdentity, String> {
    let encoder = object
        .get("encoder")
        .and_then(Value::as_object)
        .ok_or_else(|| "encoder is required".to_owned())?;
    Ok(solstone_core_entity::EncoderIdentity {
        id: required_string(encoder, "id")?,
        sha256: required_string(encoder, "sha256")?,
        width: encoder
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "encoder width is required".to_owned())?,
    })
}

fn optional_string(object: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(format!("{name} must be a non-empty string or null")),
    }
}

fn required_bool(object: &Map<String, Value>, name: &str) -> Result<bool, String> {
    object
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{name} is required"))
}

fn required_i64(object: &Map<String, Value>, name: &str) -> Result<i64, String> {
    object
        .get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{name} is required"))
}

fn parse_accumulation_request(
    value: Value,
) -> Result<solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationRequest, String> {
    use solstone_core_entity::EncoderIdentity;
    use solstone_core_speaker_resolve::voiceprint_accumulation::{
        AccumulationEmbedding, AccumulationLabel, AccumulationRequest,
    };
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    const FIELDS: [&str; 8] = [
        "schema",
        "journal_root",
        "segment",
        "now_ms",
        "encoder",
        "labels",
        "embeddings",
        "entity_ids",
    ];
    if object.keys().any(|key| !FIELDS.contains(&key.as_str()))
        || object.get("schema").and_then(Value::as_str)
            != Some("solstone-speaker-resolve-accumulate-voiceprints-request-v1")
    {
        return Err("invalid accumulation request schema".to_owned());
    }
    let segment = object
        .get("segment")
        .and_then(Value::as_object)
        .ok_or_else(|| "segment is required".to_owned())?;
    let string = |object: &Map<String, Value>, key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is required"))
    };
    let encoder = object
        .get("encoder")
        .and_then(Value::as_object)
        .ok_or_else(|| "encoder is required".to_owned())?;
    let labels = serde_json::from_value::<Vec<AccumulationLabel>>(
        object
            .get("labels")
            .cloned()
            .ok_or_else(|| "labels is required".to_owned())?,
    )
    .map_err(|_| "invalid labels".to_owned())?;
    let embeddings = serde_json::from_value::<Vec<AccumulationEmbedding>>(
        object
            .get("embeddings")
            .cloned()
            .ok_or_else(|| "embeddings is required".to_owned())?,
    )
    .map_err(|_| "invalid embeddings".to_owned())?;
    let entity_ids = serde_json::from_value::<Vec<String>>(
        object
            .get("entity_ids")
            .cloned()
            .ok_or_else(|| "entity_ids is required".to_owned())?,
    )
    .map_err(|_| "invalid entity_ids".to_owned())?;
    Ok(AccumulationRequest {
        journal_root: PathBuf::from(string(object, "journal_root")?),
        day: string(segment, "day")?,
        stream: string(segment, "stream")?,
        segment_key: string(segment, "segment_key")?,
        source: string(segment, "source")?,
        now_ms: object
            .get("now_ms")
            .and_then(Value::as_i64)
            .ok_or_else(|| "now_ms is required".to_owned())?,
        encoder: EncoderIdentity {
            id: string(encoder, "id")?,
            sha256: string(encoder, "sha256")?,
            width: object
                .get("encoder")
                .and_then(|value| value.get("width"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| "encoder width is required".to_owned())?,
        },
        labels,
        embeddings,
        entity_ids,
    })
}

fn write_owner_centroid_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::owner_centroid::{
        OwnerCentroidWriteInput, write_owner_centroid,
    };
    let fields = owner_fields(value)?;
    write_owner_centroid(
        &fields.root,
        &fields.principal,
        &OwnerCentroidWriteInput {
            centroid: fields.centroid,
            cluster_size: fields.cluster_size,
            timestamp: fields.timestamp,
            evidence_tier: fields.evidence_tier,
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn rebuild_owner_centroid_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::owner_centroid::{
        OwnerCentroidRebuildInput, rebuild_owner_centroid,
    };
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    let fields = owner_fields(Value::Object(object.clone()))?;
    let evidence_hash = object
        .get("evidence_hash")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "evidence_hash is required".to_owned())?
        .to_owned();
    let p25 = object
        .get("evidence_intra_cosine_p25")
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .ok_or_else(|| "evidence_intra_cosine_p25 is required".to_owned())?;
    let outcome = rebuild_owner_centroid(
        &fields.root,
        &fields.principal,
        &OwnerCentroidRebuildInput {
            centroid: fields.centroid,
            embeddings_count: fields.cluster_size,
            timestamp: fields.timestamp,
            evidence_hash,
            evidence_intra_cosine_p25: p25,
            evidence_tier: fields.evidence_tier,
            override_regression: object
                .get("override")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
    )
    .map_err(|error| error.to_string())?;
    let outcome = serde_json::to_value(outcome).map_err(|error| error.to_string())?;
    Ok(json!({"outcome": outcome}))
}

fn write_owner_candidate_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::owner_candidate::{OwnerCandidate, write_owner_candidate};
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let candidate = OwnerCandidate {
        centroid: f32_values(object.get("centroid"))?,
        cluster_size: required_i32(object, "cluster_size")?,
        threshold: required_f32(object, "threshold")?,
        version: required_string(object, "version")?,
        evidence_tier: required_string(object, "evidence_tier")?,
    };
    write_owner_candidate(&root, &candidate).map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn read_owner_candidate_request(value: Value) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    let candidate = solstone_core_speaker_resolve::owner_candidate::load_owner_candidate(
        &PathBuf::from(required_string(object, "journal_root")?),
    )
    .map_err(|error| error.to_string())?;
    let candidate = serde_json::to_value(candidate).map_err(|error| error.to_string())?;
    Ok(json!({"candidate": candidate}))
}

fn clear_owner_candidate_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-clear-owner-candidate-request-v1",
        &["schema", "journal_root"],
    )?;
    let removed = solstone_core_speaker_resolve::owner_candidate::clear_owner_candidate(
        &PathBuf::from(required_string(&object, "journal_root")?),
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "status":"cleared",
        "removed":matches!(removed, solstone_core_journal_io::Removed::Unlinked),
    }))
}

fn write_voiceprint_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-write-voiceprint-request-v1",
        &[
            "schema",
            "journal_root",
            "entity_id",
            "embedding",
            "metadata",
            "encoder",
        ],
    )?;
    let metadata = required_object_value(&object, "metadata")?;
    solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
        &PathBuf::from(required_string(&object, "journal_root")?),
        &required_string(&object, "entity_id")?,
        f32_values_named(object.get("embedding"), "embedding")?,
        Value::Object(metadata),
        &encoder(&object)?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn remove_voiceprint_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-remove-voiceprint-request-v1",
        &["schema", "journal_root", "entity_id", "key", "encoder"],
    )?;
    let key = required_object_value(&object, "key")?;
    let report = solstone_core_speaker_resolve::direct_voiceprints::remove_voiceprint(
        &PathBuf::from(required_string(&object, "journal_root")?),
        &required_string(&object, "entity_id")?,
        Value::Object(key.clone()),
        &encoder(&object)?,
    )
    .map_err(|error| error.to_string())?;
    let outcome = if report.removed_count == 0 {
        "not_found"
    } else if report.file_removed {
        "unlinked"
    } else {
        "rewritten"
    };
    Ok(json!({
        "outcome":outcome,
        "entity_id":required_string(&object, "entity_id")?,
        "keys_removed":if report.removed_count == 0 { Vec::<Value>::new() } else { vec![Value::Object(key)] },
        "file_deleted":report.file_removed,
    }))
}

fn backfill_voiceprint_last_seen_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-backfill-voiceprint-last-seen-request-v1",
        &[
            "schema",
            "journal_root",
            "entity_id",
            "last_seen_ts",
            "encoder",
        ],
    )?;
    let target_ts = required_i64(&object, "last_seen_ts")?;
    let updates = solstone_core_entity::rewrite_voiceprint_metadata(
        &PathBuf::from(required_string(&object, "journal_root")?),
        &required_string(&object, "entity_id")?,
        &encoder(&object)?,
        |rows| {
            let mut changed = 0;
            for row in rows {
                let Some(metadata) = row.as_object_mut() else {
                    continue;
                };
                let current = metadata.get("last_seen_ts").and_then(Value::as_i64);
                if current.is_none_or(|current| current < target_ts) {
                    metadata.insert("last_seen_ts".to_owned(), Value::from(target_ts));
                    changed += 1;
                }
            }
            changed
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"rows_written":updates}))
}

fn write_stub_labels_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-write-stub-labels-request-v1",
        &["schema", "journal_root", "segment", "reason"],
    )?;
    solstone_core_speaker_id::labels::write_stub_labels(
        &segment_dir(&object)?,
        &required_string(&object, "reason")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn write_full_labels_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-write-full-labels-request-v1",
        &["schema", "journal_root", "segment", "labels", "metadata"],
    )?;
    let labels = object
        .get("labels")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "labels is required".to_owned())?;
    solstone_core_speaker_id::labels::write_full_labels(
        &segment_dir(&object)?,
        labels,
        &required_object_value(&object, "metadata")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn patch_labels_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-patch-labels-request-v1",
        &[
            "schema",
            "journal_root",
            "segment",
            "patches",
            "allow_insert",
        ],
    )?;
    let patches = object
        .get("patches")
        .and_then(Value::as_object)
        .ok_or_else(|| "patches is required".to_owned())?
        .iter()
        .map(|(sentence_id, fields)| {
            Ok((
                sentence_id
                    .parse::<i64>()
                    .map_err(|_| "patch sentence ID must be an integer".to_owned())?,
                fields
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "patch fields must be an object".to_owned())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    solstone_core_speaker_id::labels::patch_labels(
        &segment_dir(&object)?,
        &patches,
        required_bool(&object, "allow_insert")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn restore_label_rows_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-restore-label-rows-request-v1",
        &["schema", "journal_root", "segment", "restorations"],
    )?;
    let restorations = object
        .get("restorations")
        .and_then(Value::as_array)
        .ok_or_else(|| "restorations is required".to_owned())?
        .iter()
        .map(parse_label_restoration)
        .collect::<Result<Vec<_>, _>>()?;
    let report =
        solstone_core_speaker_id::labels::restore_label_rows(&segment_dir(&object)?, &restorations)
            .map_err(|error| error.to_string())?;
    Ok(json!({
        "restored_count":report.restored_count,
        "removed_inserted_count":report.removed_inserted_count,
        "patched_existing_count":report.patched_existing_count,
        "skipped_count":report.skipped_count,
        "skipped_reasons":{"missing":report.missing_count,"changed":report.changed_count},
    }))
}

fn append_correction_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-append-correction-request-v1",
        &["schema", "journal_root", "segment", "correction"],
    )?;
    solstone_core_speaker_id::corrections::append_correction(
        &segment_dir(&object)?,
        required_object_value(&object, "correction")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"appended"}))
}

fn wipe_speaker_artifacts_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-wipe-speaker-artifacts-request-v1",
        &["schema", "journal_root", "dry_run"],
    )?;
    let report = solstone_core_speaker_resolve::artifact_wipe::wipe_speaker_artifacts(
        &PathBuf::from(required_string(&object, "journal_root")?),
        required_bool(&object, "dry_run")?,
    )
    .map_err(|error| error.to_string())?;
    serde_json::to_value(report).map_err(|error| error.to_string())
}

fn segment_dir(object: &Map<String, Value>) -> Result<PathBuf, String> {
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let segment = object
        .get("segment")
        .and_then(Value::as_object)
        .ok_or_else(|| "segment is required".to_owned())?;
    const FIELDS: [&str; 3] = ["day", "stream", "segment_key"];
    if segment.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err("invalid segment request".to_owned());
    }
    let day = required_string(segment, "day")?;
    let stream = required_string(segment, "stream")?;
    let segment_key = required_string(segment, "segment_key")?;
    solstone_core_journal_io::contained_path(
        &root,
        &format!("chronicle/{day}/{stream}/{segment_key}"),
    )
    .map_err(|error| error.to_string())
}

fn required_object_value(
    object: &Map<String, Value>,
    name: &str,
) -> Result<Map<String, Value>, String> {
    object
        .get(name)
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| format!("{name} is required"))
}

fn parse_label_restoration(
    value: &Value,
) -> Result<solstone_core_speaker_id::labels::LabelRestoration, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "label restoration must be an object".to_owned())?;
    const FIELDS: [&str; 4] = [
        "sentence_id",
        "expected_current_label",
        "prior_state",
        "prior_label",
    ];
    if object.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err("invalid label restoration".to_owned());
    }
    Ok(solstone_core_speaker_id::labels::LabelRestoration {
        sentence_id: required_i64(object, "sentence_id")?,
        expected_current_label: object
            .get("expected_current_label")
            .cloned()
            .ok_or_else(|| "expected_current_label is required".to_owned())?,
        prior_state: required_string(object, "prior_state")?,
        prior_label: object.get("prior_label").cloned(),
    })
}

struct OwnerFields {
    root: PathBuf,
    principal: String,
    centroid: Vec<f32>,
    cluster_size: i32,
    timestamp: String,
    evidence_tier: String,
}

fn owner_fields(value: Value) -> Result<OwnerFields, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    Ok(OwnerFields {
        root: PathBuf::from(required_string(object, "journal_root")?),
        principal: required_string(object, "principal_entity_id")?,
        centroid: f32_values(object.get("centroid"))?,
        cluster_size: required_i32(object, "cluster_size")?,
        timestamp: required_string(object, "timestamp")?,
        evidence_tier: required_string(object, "evidence_tier")?,
    })
}

fn required_string(object: &Map<String, Value>, name: &str) -> Result<String, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} is required"))
}
fn required_i32(object: &Map<String, Value>, name: &str) -> Result<i32, String> {
    object
        .get(name)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("{name} is required"))
}
fn required_f32(object: &Map<String, Value>, name: &str) -> Result<f32, String> {
    object
        .get(name)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{name} is required"))
}
fn f32_values(value: Option<&Value>) -> Result<Vec<f32>, String> {
    f32_values_named(value, "centroid")
}
fn f32_values_named(value: Option<&Value>, name: &str) -> Result<Vec<f32>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{name} is required"))?
        .iter()
        .map(|value| {
            value
                .as_f64()
                .map(|value| value as f32)
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("invalid {name} float values"))
        })
        .collect()
}

fn run_speaker_transcript_write() -> ExitCode {
    let mut input = Vec::new();
    let read_result = io::stdin()
        .lock()
        .take((MAX_JSON_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut input);
    if let Err(error) = read_result {
        eprint_speaker_transcript_write_error("internal-error", &error.to_string());
        return ExitCode::from(EXIT_TEMPFAIL);
    }
    if input.len() > MAX_JSON_STDIN_BYTES {
        eprint_speaker_transcript_write_error(
            "malformed-request",
            "stdin request exceeds the JSON input limit",
        );
        return ExitCode::from(EXIT_USAGE);
    }
    match solstone_core_speaker_id::writer::write_request(&input) {
        Ok(response) => {
            let output = json!({
                "schema": solstone_core_speaker_id::writer::RESPONSE_SCHEMA,
                "jsonl_path": response.jsonl_path,
                "npz_path": response.npz_path,
                "statement_count": response.statement_count,
                "embedding_row_count": response.embedding_row_count,
            });
            match serde_json::to_string(&output) {
                Ok(output) => {
                    println!("{output}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprint_speaker_transcript_write_error("internal-error", &error.to_string());
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        }
        Err(error) => {
            let exit = speaker_transcript_write_exit_code(&error);
            eprint_speaker_transcript_write_error(error.reason(), &error.detail());
            ExitCode::from(exit)
        }
    }
}

fn speaker_transcript_write_exit_code(
    error: &solstone_core_speaker_id::writer::SpeakerTranscriptWriteError,
) -> u8 {
    use solstone_core_speaker_id::writer::SpeakerTranscriptWriteError;

    match error {
        SpeakerTranscriptWriteError::MalformedRequest { .. }
        | SpeakerTranscriptWriteError::UnknownSchema { .. }
        | SpeakerTranscriptWriteError::MissingStatementId { .. }
        | SpeakerTranscriptWriteError::InvalidStatementId { .. }
        | SpeakerTranscriptWriteError::DuplicateStatementId { .. }
        | SpeakerTranscriptWriteError::InvalidStatement { .. }
        | SpeakerTranscriptWriteError::InvalidHeader { .. }
        | SpeakerTranscriptWriteError::InvalidOutputPath { .. }
        | SpeakerTranscriptWriteError::DestinationExists { .. } => EXIT_USAGE,
        SpeakerTranscriptWriteError::PayloadUnreadable { .. }
        | SpeakerTranscriptWriteError::PayloadInvalid { .. }
        | SpeakerTranscriptWriteError::PayloadNonFinite { .. } => EXIT_UNAVAILABLE,
        SpeakerTranscriptWriteError::OutputUnwritable { .. }
        | SpeakerTranscriptWriteError::NpzVerificationFailed { .. }
        | SpeakerTranscriptWriteError::Internal { .. } => EXIT_TEMPFAIL,
    }
}

fn eprint_speaker_transcript_write_error(reason: &str, detail: &str) {
    eprintln!(
        "{}",
        json!({
            "schema": "solstone-speaker-transcript-write-error-v1",
            "reason": reason,
            "detail": detail,
        })
    );
}

fn run_local(command: LocalCommand) -> ExitCode {
    match command {
        LocalCommand::ProbeNvidia => {
            let probe = solstone_core_local::probe_nvidia_gpu();
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            if serde_json::to_writer(&mut stdout, &probe).is_err() || writeln!(stdout).is_err() {
                return ExitCode::from(EXIT_IOERR);
            }
            ExitCode::SUCCESS
        }
        LocalCommand::Plan => run_local_json(solstone_core_local::plan),
        LocalCommand::Connect => run_local_json(solstone_core_local::connect),
        LocalCommand::Install(command) => run_local_install(command),
        LocalCommand::Generate => run_local_generate_json(solstone_core_local::generate),
    }
}

fn run_generate(command: GenerateCommand) -> ExitCode {
    match command {
        GenerateCommand::Contract => {
            print!("{}", solstone_core_generate::contract_source());
            ExitCode::SUCCESS
        }
        GenerateCommand::Malformed => generate_protocol_exit(
            None,
            "malformed-request",
            generate_selector_detail(),
            generate_exit_code("malformed_request"),
        ),
        GenerateCommand::OneShot => run_generate_one_shot(),
        GenerateCommand::Session(options) => run_generate_session(options),
    }
}

/// Run the native cogitate process boundary.
///
/// `cogitate_wire_contract.json` is the source of truth for which registered
/// Cortex kinds are native producers and which are consumer/transport-side.
/// Cortex writes a separate durable use log with consumer-added `use_id` and
/// `exit_code`, synthesized `info.message` and `error` records, and the
/// `_active.jsonl` to `.jsonl` rename (`cortex.py:707-733,1379-1384`). This
/// subcommand only streams its NDJSON boundary. It replaces the in-process
/// loop, policy, prompt assembly, and provider resolution formerly performed
/// by `talents.py`/`openhands.py`; Python still owns post-hooks, output files,
/// provenance, no-output gating, and reuse (`talents.py:1518-1619`).
fn run_cogitate(command: CogitateCommand) -> ExitCode {
    match command {
        CogitateCommand::Contract => {
            print!("{}", solstone_core_cogitate_wire::contract_source());
            ExitCode::SUCCESS
        }
        CogitateCommand::Malformed => {
            eprintln!("cogitate: expected --contract or --one-shot");
            ExitCode::from(EXIT_DATAERR)
        }
        CogitateCommand::OneShot => run_cogitate_one_shot(),
    }
}

fn run_cogitate_one_shot() -> ExitCode {
    // One-shot cogitate has one complete request, rather than generate's
    // multiplexed session framing, so a direct read-to-string is sufficient.
    let mut raw = String::new();
    if let Err(error) = io::stdin().lock().read_to_string(&mut raw) {
        eprintln!("cogitate stdin I/O error: {error}");
        return ExitCode::from(EXIT_INTERNAL_FAILURE);
    }
    let request = match solstone_core_cogitate_wire::CogitateRequest::parse(&raw) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(EXIT_DATAERR);
        }
    };
    // Dry runs deliberately avoid endpoint resolution, allowing callers to
    // validate a request without any provider configuration.
    if request.dry_run {
        return match solstone_core_cogitate_wire::serialize_dry_run(&request) {
            Ok(value) => {
                write_ndjson_line_or_exit(&value);
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(EXIT_INTERNAL_FAILURE)
            }
        };
    }
    let mut provider =
        match solstone_core_cogitate_wire::EndpointConverseProvider::from_request(&request) {
            Ok(provider) => provider,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(EXIT_PROVIDER_CONFIGURATION);
            }
        };
    let mut slot = solstone_core_cogitate_tools::NoopSlotLease;
    let mut tools = solstone_core_cogitate_runtime::CogitateToolExecutor::new(
        &request.journal_root,
        request.read_call_budget,
        &mut slot,
    );
    let mut sink = StdoutEventSink;
    match solstone_core_cogitate_wire::run_or_dry_run(
        &request,
        &mut provider,
        &mut tools,
        &mut sink,
    ) {
        Ok(solstone_core_cogitate_wire::NativeRun::Completed(_)) => ExitCode::SUCCESS,
        Ok(solstone_core_cogitate_wire::NativeRun::DryRun(value)) => {
            write_ndjson_line_or_exit(&value);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_INTERNAL_FAILURE)
        }
    }
}

struct StdoutEventSink;

impl solstone_core_cogitate_runtime::EventSink for StdoutEventSink {
    fn emit(&mut self, event: solstone_core_cogitate_runtime::RuntimeEvent) {
        match solstone_core_cogitate_wire::serialize_event_validated(event) {
            Ok(value) => write_ndjson_line_or_exit(&value),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(EXIT_INTERNAL_FAILURE.into());
            }
        }
    }
}

fn write_ndjson_line_or_exit(value: &Value) {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    if stdout
        .write_all(value.to_string().as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .is_err()
    {
        // AC #16: EventSink cannot return Result, so terminate at the exact
        // failed write. Another final event cannot reach a dead pipe; process
        // exit belongs in this binary boundary, never in a subsystem crate.
        std::process::exit(EXIT_IOERR.into());
    }
}

fn generate_selector_detail() -> String {
    let session_selector = solstone_core_generate::contract()["framing"]["session"]["selector"]
        .as_str()
        .expect("session selector is a fixture string");
    format!("expected --contract, --one-shot, or {session_selector}")
}

fn run_generate_one_shot() -> ExitCode {
    let raw = match read_generate_stdin() {
        Ok(raw) => raw,
        Err(GenerateStdinError::InvalidUtf8(detail)) => {
            return generate_protocol_exit(
                None,
                "malformed-request",
                detail,
                generate_exit_code("malformed_request"),
            );
        }
        Err(GenerateStdinError::Io(detail)) => {
            return generate_protocol_exit(
                None,
                "internal-failure",
                detail,
                generate_exit_code("internal_failure"),
            );
        }
        Err(GenerateStdinError::TooLarge) => {
            return generate_protocol_exit(
                None,
                "internal-failure",
                "stdin exceeds 64 MiB".to_owned(),
                generate_exit_code("internal_failure"),
            );
        }
    };
    let request = match solstone_core_generate_wire::parse_one_shot_request(&raw) {
        Ok(request) => request,
        Err(detail) => {
            return generate_protocol_exit(
                None,
                "malformed-request",
                detail,
                generate_exit_code("malformed_request"),
            );
        }
    };
    let request_id = request.id.clone();
    let endpoint_runtime = solstone_core_generate_wire::EndpointRuntime::default();
    let response = match generate_response_for_request(&request, &endpoint_runtime) {
        Ok(response) => response,
        Err(detail) => {
            return generate_protocol_exit(
                request_id,
                "internal-failure",
                detail,
                generate_exit_code("internal_failure"),
            );
        }
    };
    match write_generate_response(&response) {
        Ok(()) => ExitCode::from(generate_exit_code("response")),
        Err(detail) => generate_protocol_exit(
            request_id,
            "internal-failure",
            detail,
            generate_exit_code("internal_failure"),
        ),
    }
}

fn generate_response_for_request(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &solstone_core_generate_wire::EndpointRuntime,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    let request_id = request.id.clone();
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return Err(journal_path_error_detail(error)),
    };
    let config = match read_journal_config(&journal) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => return Err(format!("could not read journal config: {error}")),
    };
    let (provider, outcome) = solstone_core_generate_wire::resolve_lane(&config);
    let response = match outcome {
        solstone_core_generate_wire::LaneOutcome::BundledLocal => {
            match solstone_core_generate_wire::bundled_generate(request, &journal) {
                Ok(solstone_core_local::GenerateResult::Success(mut success)) => {
                    let usage = success
                        .usage
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(|error| error.to_string())?
                        .unwrap_or_else(|| json!({}));
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: &journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response = generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                Ok(solstone_core_local::GenerateResult::Failure(failure)) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::BundledFailure(Box::new(
                                failure,
                            )),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
                Err(error) => {
                    return Err(format!("bundled local generate is unavailable: {error:?}"));
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::ByoEndpoint(endpoint) => {
            endpoint_result_response(
                solstone_core_generate_wire::endpoint_generate(
                    request,
                    &journal,
                    &endpoint,
                    &config,
                    endpoint_runtime,
                ),
                request,
                &journal,
                &provider,
                request_id.clone(),
            )?
        }
        solstone_core_generate_wire::LaneOutcome::ConfidentialEndpoint(endpoint) => {
            match solstone_core_generate_wire::confidential_generate(
                request,
                &journal,
                &endpoint,
                &config,
                endpoint_runtime,
            ) {
                solstone_core_generate_wire::ConfidentialResult::Generated(success) => {
                    endpoint_result_response(
                        solstone_core_generate_wire::EndpointResult::Generated(success),
                        request,
                        &journal,
                        &provider,
                        request_id.clone(),
                    )?
                }
                solstone_core_generate_wire::ConfidentialResult::Failed(failure) => {
                    endpoint_result_response(
                        solstone_core_generate_wire::EndpointResult::Failed(failure),
                        request,
                        &journal,
                        &provider,
                        request_id.clone(),
                    )?
                }
                solstone_core_generate_wire::ConfidentialResult::AttestationNotVerified => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AttestationNotVerified,
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
                solstone_core_generate_wire::ConfidentialResult::AttestationFailed => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AttestationFailed,
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
                solstone_core_generate_wire::ConfidentialResult::AttestationStale => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AttestationStale,
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::Anthropic => {
            match solstone_core_generate_wire::anthropic_generate(request, &config) {
                solstone_core_generate_wire::AnthropicResult::Generated(mut success) => {
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: &journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &success.usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            anthropic_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                solstone_core_generate_wire::AnthropicResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AnthropicFailure(failure),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::OpenAi => {
            match solstone_core_generate_wire::openai_generate(request, &config) {
                solstone_core_generate_wire::OpenAiResult::Generated(mut success) => {
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: &journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &success.usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            openai_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                solstone_core_generate_wire::OpenAiResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::OpenAiFailure(failure),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::Google => {
            match solstone_core_generate_wire::google_generate(request, &config) {
                solstone_core_generate_wire::GoogleResult::Generated(mut success) => {
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: &journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &success.usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            google_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                solstone_core_generate_wire::GoogleResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::GoogleFailure(failure),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::NoEngine
        | solstone_core_generate_wire::LaneOutcome::AttestationNotVerified
        | solstone_core_generate_wire::LaneOutcome::AttestationFailed
        | solstone_core_generate_wire::LaneOutcome::AttestationStale
        | solstone_core_generate_wire::LaneOutcome::UnimplementedLane => {
            solstone_core_generate::GenerateResponse::Refused(
                solstone_core_generate_wire::refusal_for(&outcome, &provider, request_id.clone()),
            )
        }
        solstone_core_generate_wire::LaneOutcome::BundledFailure(_)
        | solstone_core_generate_wire::LaneOutcome::EndpointFailure(_)
        | solstone_core_generate_wire::LaneOutcome::AnthropicFailure(_)
        | solstone_core_generate_wire::LaneOutcome::OpenAiFailure(_)
        | solstone_core_generate_wire::LaneOutcome::GoogleFailure(_)
        | solstone_core_generate_wire::LaneOutcome::ValidationFailure(_) => {
            unreachable!("lane resolution cannot return an arm failure")
        }
    };
    Ok(response)
}

fn endpoint_result_response(
    result: solstone_core_generate_wire::EndpointResult,
    request: &solstone_core_generate::GenerateRequest,
    journal: &std::path::Path,
    provider: &str,
    request_id: Option<String>,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    match result {
        solstone_core_generate_wire::EndpointResult::Generated(mut success) => {
            let usage = success
                .usage
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|error| error.to_string())?
                .unwrap_or_else(|| json!({}));
            let assessment = solstone_core_generate_wire::assess_provider_result(
                solstone_core_generate_wire::ProviderResultView {
                    journal_path: journal,
                    context: &request.context,
                    model: &success.model,
                    text: &success.text,
                    finish_reason: &success.finish_reason,
                    usage: &usage,
                    json_output: request.json_output,
                    enforce_responsiveness: request.enforce_responsiveness,
                },
            );
            if let Some(error) = assessment.token_log_error {
                eprintln!("generate token usage log failed: {error}");
            }
            if let Some(failure) = assessment.failure {
                Ok(solstone_core_generate::GenerateResponse::Refused(
                    solstone_core_generate_wire::refusal_for(
                        &solstone_core_generate_wire::LaneOutcome::ValidationFailure(failure),
                        provider,
                        request_id,
                    ),
                ))
            } else {
                let schema_validation =
                    apply_schema_validation(&mut success.text, request.json_schema.as_ref());
                let response = endpoint_generated_response(request, success, schema_validation)?;
                Ok(solstone_core_generate::GenerateResponse::Generated(
                    Box::new(response),
                ))
            }
        }
        solstone_core_generate_wire::EndpointResult::Failed(failure) => {
            Ok(solstone_core_generate::GenerateResponse::Refused(
                solstone_core_generate_wire::refusal_for(
                    &solstone_core_generate_wire::LaneOutcome::EndpointFailure(failure),
                    provider,
                    request_id,
                ),
            ))
        }
    }
}

fn run_generate_session(options: GenerateSessionOptions) -> ExitCode {
    let config = match generate_session_config(&options) {
        Ok(config) => config,
        Err(detail) => {
            return generate_protocol_exit(
                None,
                "malformed-request",
                detail,
                generate_exit_code("malformed_request"),
            );
        }
    };
    // The framing itself lives in the wire crate. What stays here is what only
    // the binary can decide: which lane answers a request, and what the process
    // does when the protocol cannot continue.
    let outcome = solstone_core_generate_wire::run_session(
        io::BufReader::new(io::stdin()),
        Box::new(io::stdout()),
        config,
        solstone_core_generate_wire::SessionHost {
            respond: generate_response_for_request,
            fail: |id, reason, detail| generate_protocol_exit_and_terminate(id, reason, detail),
            // Bare EOF means the caller disappeared: answer nothing further, write no
            // further usage, exit. The contract declares 0, 64 and 70 only, and the
            // reference implementation returns from its session loop here, so this exits 0.
            // Whether the abort deserves a distinct declared code is a contract question,
            // not one a port settles: 1 would collide with crash, kill and OOM, which is
            // the same collision the absent 69 exists to avoid.
            abort: || std::process::exit(0),
        },
    );
    match outcome {
        solstone_core_generate_wire::SessionOutcome::Completed => {
            ExitCode::from(generate_exit_code("response"))
        }
        solstone_core_generate_wire::SessionOutcome::ReaderDisconnected => generate_protocol_exit(
            None,
            "internal-failure",
            "session stdin reader disconnected".to_owned(),
            generate_exit_code("internal_failure"),
        ),
    }
}

fn generate_session_config(
    options: &GenerateSessionOptions,
) -> Result<solstone_core_generate_wire::SessionConfig, String> {
    let session = &solstone_core_generate::contract()["framing"]["session"];
    let selector = session["selector"]
        .as_str()
        .ok_or_else(|| "session selector is not a string".to_owned())?;
    let concurrency = &session["concurrency"];
    let flag = concurrency["flag"]
        .as_str()
        .ok_or_else(|| "session concurrency flag is not a string".to_owned())?;
    let minimum = concurrency["minimum"]
        .as_u64()
        .ok_or_else(|| "session concurrency minimum is not an integer".to_owned())?;
    let line_limit_bytes = session["line_limit_bytes"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "session line limit is not a supported integer".to_owned())?;
    let terminal_schema = session["terminal"]["schema"]
        .as_str()
        .ok_or_else(|| "session terminal schema is not a string".to_owned())?
        .to_owned();
    let [actual_flag, value] = options.arguments.as_slice() else {
        return Err(format!("expected {selector} {flag} <positive integer>"));
    };
    if actual_flag != std::ffi::OsStr::new(flag) {
        return Err(format!("expected {selector} {flag} <positive integer>"));
    }
    let value = value
        .to_str()
        .ok_or_else(|| format!("expected {selector} {flag} <positive integer>"))?;
    let max_in_flight = value
        .parse::<usize>()
        .map_err(|_| format!("expected {selector} {flag} <positive integer>"))?;
    let minimum = usize::try_from(minimum)
        .map_err(|_| "session concurrency minimum is not a supported integer".to_owned())?;
    if max_in_flight < minimum {
        return Err(format!(
            "{flag} is below the fixture minimum of {minimum} for {selector}"
        ));
    }
    Ok(solstone_core_generate_wire::SessionConfig {
        max_in_flight,
        line_limit_bytes,
        terminal_schema,
    })
}

fn generate_protocol_exit_and_terminate(id: Option<String>, reason: &str, detail: String) -> ! {
    let exit = generate_exit_code(if reason == "malformed-request" {
        "malformed_request"
    } else {
        "internal_failure"
    });
    let _ = generate_protocol_exit(id, reason, detail, exit);
    std::process::exit(i32::from(exit));
}

fn generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_local::GenerateSuccess,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let usage = success
        .usage
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| json!({}));
    let input_budget = success
        .input_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let request_budget = serde_json::to_value(&success.request_budget)
        .map(Some)
        .map_err(|error| error.to_string())?;
    let inference = serde_json::to_value(&success.inference)
        .map(Some)
        .map_err(|error| error.to_string())?;
    let hints_applied = applied_hints(request, true, true);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage,
        finish_reason: success.finish_reason,
        thinking: None,
        schema_validation,
        input_budget,
        request_budget,
        inference,
        hints_applied,
    })
}

/// Which request hints the boundary actually applied.
///
/// 🔴 A hint is reported only by a lane that HONOURED it, and the lanes differ:
/// measured, `bundled` reads `exclusive_admission` and the attempt index,
/// `endpoint` reads `exclusive_admission` to size its admission slot, and the
/// four cloud lanes read neither — their only mention of it is a test fixture.
///
/// ⚠ Two wrong rules were tried before this one, and both passed a real test
/// suite. The original reported `exclusive_admission` from every lane, so the
/// cloud lanes claimed an effect they never had. The correction after that keyed
/// off the response carrying an inference block, which is how the Python
/// reference decided it — but that gates on the *shape of the result*, not on
/// what was honoured, and it silenced the endpoint lane, which genuinely does
/// acquire the slot. ⛔ Do not re-derive this from the reference; it
/// under-reported.
fn applied_hints(
    request: &solstone_core_generate::GenerateRequest,
    honours_attempt_index: bool,
    honours_exclusive_admission: bool,
) -> Vec<String> {
    let mut hints = Vec::new();
    if honours_attempt_index && request.attempt_index != 0 {
        hints.push("attempt_index".to_owned());
    }
    if honours_exclusive_admission && request.exclusive_admission {
        hints.push("exclusive_admission".to_owned());
    }
    hints
}

fn endpoint_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::EndpointGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let usage = success
        .usage
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| json!({}));
    let input_budget = success
        .input_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let request_budget = success
        .request_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let hints_applied = applied_hints(request, false, true);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage,
        finish_reason: success.finish_reason,
        thinking: None,
        schema_validation,
        input_budget,
        request_budget,
        inference: None,
        hints_applied,
    })
}

fn anthropic_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::AnthropicGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn openai_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::OpenAiGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn google_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::GoogleGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn apply_schema_validation(text: &mut String, schema: Option<&Value>) -> Option<Value> {
    schema.map(|schema| {
        let result = solstone_core_generate_wire::validate_schema_with_annotations(text, schema);
        *text = result.text;
        log_schema_validation(&result.validation);
        result.validation
    })
}

/// Schema validation is advisory, so a failure never changes the outcome and is
/// visible only in the response record — which reaches the caller and nothing
/// else.
///
/// ⚠ The reference emitted one operator-visible line per error and the port
/// dropped it. Nothing failed: the record still carried the block, so the
/// contract-level assertions passed and only the diagnostic disappeared. ⛔ On
/// stderr, never stdout — a line on stdout would corrupt the response stream,
/// which is what the test guarding this is named for.
fn log_schema_validation(validation: &Value) {
    let Some(errors) = validation.get("errors").and_then(Value::as_array) else {
        return;
    };
    for error in errors {
        let path = error.get("path").and_then(Value::as_str).unwrap_or("");
        let constraint = error
            .get("constraint")
            .and_then(Value::as_str)
            .unwrap_or("");
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        eprintln!("schema_validation: {path}: {constraint}: {message}");
    }
}

enum GenerateStdinError {
    Io(String),
    TooLarge,
    InvalidUtf8(String),
}

fn read_generate_stdin() -> Result<String, GenerateStdinError> {
    let mut stdin = io::stdin().lock();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin
            .read(&mut chunk)
            .map_err(|error| GenerateStdinError::Io(format!("stdin I/O error: {error}")))?;
        if read == 0 {
            break;
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|next| next > MAX_LOCAL_GENERATE_STDIN_BYTES)
        {
            return Err(GenerateStdinError::TooLarge);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(bytes)
        .map_err(|error| GenerateStdinError::InvalidUtf8(format!("stdin is not UTF-8: {error}")))
}

fn generate_exit_code(name: &str) -> u8 {
    solstone_core_generate::contract()["exit_codes"][name]
        .as_u64()
        .expect("generate contract exit code is an unsigned integer") as u8
}

fn generate_protocol_exit(id: Option<String>, reason: &str, detail: String, exit: u8) -> ExitCode {
    let error = solstone_core_generate::ProtocolError {
        id,
        reason: reason.to_owned(),
        detail,
    };
    match solstone_core_generate::encode_protocol_error(&error) {
        Ok(encoded) => {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(encoded.as_bytes());
            let _ = stderr.write_all(b"\n");
            let _ = stderr.flush();
        }
        Err(error) => eprintln!("generate protocol error encoding failed: {error}"),
    }
    ExitCode::from(exit)
}

fn write_generate_response(
    response: &solstone_core_generate::GenerateResponse,
) -> Result<(), String> {
    let encoded = solstone_core_generate::encode_one_shot_response(response)?;
    write_generate_response_to(&mut io::stdout().lock(), &encoded)
}

fn write_generate_response_to(writer: &mut impl Write, encoded: &str) -> Result<(), String> {
    writer
        .write_all(encoded.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|error| format!("stdout I/O error: {error}"))
}

fn journal_path_error_detail(error: JournalPathError) -> String {
    match error {
        JournalPathError::Config(error) => format!("could not resolve journal path: {error:?}"),
        JournalPathError::Home(error) => format!("could not resolve journal path: {error:?}"),
        JournalPathError::Create(error) => format!("could not resolve journal path: {error}"),
    }
}

fn run_local_install(command: InstallCommand) -> ExitCode {
    let input = match read_local_stdin(MAX_LOCAL_STDIN_BYTES) {
        Ok(input) => input,
        Err(LocalStdinError::Content) => return ExitCode::from(EXIT_USAGE),
        Err(LocalStdinError::Io) => return ExitCode::from(EXIT_IOERR),
    };
    let verb = match command {
        InstallCommand::PinsLocal => solstone_core_local::InstallVerb::PinsLocal,
        InstallCommand::PathsLocal => solstone_core_local::InstallVerb::PathsLocal,
        InstallCommand::FingerprintLocal => solstone_core_local::InstallVerb::FingerprintLocal,
        InstallCommand::FingerprintMlx => solstone_core_local::InstallVerb::FingerprintMlx,
        InstallCommand::VerifySha256 => solstone_core_local::InstallVerb::VerifySha256,
        InstallCommand::CudaTrust => solstone_core_local::InstallVerb::CudaTrust,
        InstallCommand::ManifestVulkan => solstone_core_local::InstallVerb::ManifestVulkan,
        InstallCommand::ManifestCuda => solstone_core_local::InstallVerb::ManifestCuda,
        InstallCommand::ManifestModel => solstone_core_local::InstallVerb::ManifestModel,
        InstallCommand::InspectLocal => solstone_core_local::InstallVerb::InspectLocal,
        InstallCommand::InspectMlx => solstone_core_local::InstallVerb::InspectMlx,
        InstallCommand::ProbeBinary => solstone_core_local::InstallVerb::ProbeBinary,
        InstallCommand::RunLocal => solstone_core_local::InstallVerb::RunLocal,
        InstallCommand::RunMlx => solstone_core_local::InstallVerb::RunMlx,
        InstallCommand::PinsParakeet => solstone_core_local::InstallVerb::PinsParakeet,
        InstallCommand::PathsParakeet => solstone_core_local::InstallVerb::PathsParakeet,
        InstallCommand::FingerprintParakeet => {
            solstone_core_local::InstallVerb::FingerprintParakeet
        }
        InstallCommand::RunParakeet => solstone_core_local::InstallVerb::RunParakeet,
    };
    match solstone_core_local::dispatch_install(verb, input) {
        Ok(envelope) => write_install_envelope(&envelope, ExitCode::SUCCESS),
        Err(error) => write_install_envelope(&error.envelope, ExitCode::from(error.exit_code)),
    }
}

fn write_install_envelope(
    envelope: &solstone_core_local::InstallEnvelope,
    exit: ExitCode,
) -> ExitCode {
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, envelope).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(EXIT_IOERR);
    }
    exit
}

fn run_local_json<T, O>(operation: impl FnOnce(T) -> O) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    run_local_json_with_limit(operation, MAX_LOCAL_STDIN_BYTES, "1 MiB")
}

fn run_local_generate_json<T, O>(operation: impl FnOnce(T) -> O) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    run_local_json_with_limit(operation, MAX_LOCAL_GENERATE_STDIN_BYTES, "64 MiB")
}

fn run_local_json_with_limit<T, O>(
    operation: impl FnOnce(T) -> O,
    max_bytes: usize,
    limit_label: &str,
) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    let input = match read_local_stdin(max_bytes) {
        Ok(input) => input,
        Err(LocalStdinError::Content) => {
            eprintln!("local command failed: stdin was not valid JSON within {limit_label}");
            return ExitCode::from(EXIT_USAGE);
        }
        Err(LocalStdinError::Io) => {
            eprintln!("local command failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &operation(input)).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

enum LocalStdinError {
    Content,
    Io,
}

fn read_local_stdin<T: DeserializeOwned>(max_bytes: usize) -> Result<T, LocalStdinError> {
    let mut stdin = io::stdin().lock();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin.read(&mut chunk).map_err(|_| LocalStdinError::Io)?;
        if read == 0 {
            break;
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|next| next > max_bytes)
        {
            return Err(LocalStdinError::Content);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    serde_json::from_slice(&bytes).map_err(|_| LocalStdinError::Content)
}

fn run_journal_config(command: JournalConfigCommand) -> ExitCode {
    match command {
        JournalConfigCommand::Read(options) => run_journal_config_read(options),
        JournalConfigCommand::Commit(options) => run_journal_config_commit(options),
    }
}

fn run_brain(command: BrainCommand) -> ExitCode {
    match command {
        BrainCommand::RecordRuntimeFailure(options) => run_brain_runtime_failure(options),
        BrainCommand::RefreshSession(options) => run_brain_refresh_session(options),
        BrainCommand::PrerequisiteRenewalSession(options) => {
            run_brain_prerequisite_renewal_session(options)
        }
        BrainCommand::Inspect(options) => run_brain_inspect(options),
        BrainCommand::Fingerprint => run_brain_fingerprint(),
    }
}

fn run_brain_inspect(options: BrainInspectOptions) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let config = match read_journal_config(&line.path) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            eprintln!("brain inspect failed: could not read journal config: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    let inspection =
        solstone_core_brain::inspect_brain_state(&line.path, &config, chrono::Utc::now());
    let active_fingerprint = active_brain_fingerprint(
        &line.path,
        &config,
        options.bundled_runtime_fingerprint_sha256,
    );
    // `inspection` is the transport form of Python's `inspect_brain_state`;
    // `active_fingerprint` serves both of its active-fingerprint read callers.
    let output = json!({
        "status": inspection_status_name(&inspection.status),
        "path": solstone_core_brain::brain_state_path(&line.path),
        "record": inspection.record,
        "projection": {
            "aggregate_state": inspection.projection.aggregate_state,
            "reason_code": inspection.projection.reason_code,
            "active_lane": inspection.projection.active_lane,
            "active_provider": inspection.projection.active_provider,
            "active_model": inspection.projection.active_model,
            "fingerprint_sha256": inspection.projection.fingerprint_sha256,
            "runtime_transition_in_progress": inspection.projection.runtime_transition_in_progress,
        },
        "reason_code": inspection.projection.reason_code,
        "error": inspection.error,
        "active_fingerprint": active_fingerprint,
    });
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        eprintln!("brain inspect failed: stdout I/O error");
        ExitCode::from(EXIT_IOERR)
    } else {
        ExitCode::SUCCESS
    }
}

fn inspection_status_name(status: &solstone_core_brain::InspectionStatus) -> &'static str {
    match status {
        solstone_core_brain::InspectionStatus::Ok => "ok",
        solstone_core_brain::InspectionStatus::Corrupt => "corrupt",
        solstone_core_brain::InspectionStatus::Unavailable => "unavailable",
    }
}

fn active_brain_fingerprint(
    journal_path: &std::path::Path,
    config: &Map<String, Value>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Value {
    let key = solstone_core_brain::load_existing_fingerprint_key(journal_path);
    brain_fingerprint_result(
        config,
        key.as_ref().map(|key| key.as_slice()),
        bundled_runtime_fingerprint_sha256,
    )
}

fn brain_fingerprint_result(
    config: &Map<String, Value>,
    hmac_key: Option<&[u8]>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Value {
    let resolution = solstone_core_brain::derive_active_brain_lane(config);
    let mut diagnostic = Map::new();
    let bundled_runtime = (resolution.lane.as_deref() == Some("bundled"))
        .then_some(bundled_runtime_fingerprint_sha256)
        .flatten();
    let (ok, fingerprint_sha256, reason_code) = match hmac_key {
        None => (false, None, Some("fingerprint_key_unavailable".to_owned())),
        Some(key) => match solstone_core_brain::build_active_brain_fingerprint(
            config,
            key,
            bundled_runtime.clone().map(Value::String),
        ) {
            Ok(Some(fingerprint)) => (true, Some(fingerprint), None),
            Ok(None) => (false, None, Some("fingerprint_not_available".to_owned())),
            Err(error) => {
                if error.0 == "configuration_invalid" {
                    diagnostic.insert(
                        "field".to_owned(),
                        Value::String("providers.active.provider".to_owned()),
                    );
                }
                (false, None, Some(error.0))
            }
        },
    };
    json!({
        "ok": ok,
        "fingerprint_sha256": fingerprint_sha256,
        "active_lane": resolution.lane,
        "active_provider": resolution.provider,
        "active_model": resolution.model,
        "reason_code": reason_code,
        "diagnostic": diagnostic,
        "bundled_runtime_fingerprint_sha256": bundled_runtime,
    })
}

struct BrainFingerprintRequest {
    config: Map<String, Value>,
    hmac_key: [u8; 32],
    bundled_runtime_fingerprint_sha256: Option<String>,
}

fn run_brain_fingerprint() -> ExitCode {
    let request = match read_bounded_json_stdin().and_then(parse_brain_fingerprint_request) {
        Ok(request) => request,
        Err(JsonStdinError::Content) => {
            eprintln!(
                "brain fingerprint failed: stdin was not a valid request JSON object within 1 MiB"
            );
            return ExitCode::from(EXIT_USAGE);
        }
        Err(JsonStdinError::Io) => {
            eprintln!("brain fingerprint failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let output = brain_fingerprint_result(
        &request.config,
        Some(&request.hmac_key),
        request.bundled_runtime_fingerprint_sha256,
    );
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        eprintln!("brain fingerprint failed: stdout I/O error");
        ExitCode::from(EXIT_IOERR)
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_brain_fingerprint_request(
    request: Map<String, Value>,
) -> Result<BrainFingerprintRequest, JsonStdinError> {
    const FIELDS: [&str; 3] = [
        "config",
        "hmac_key_hex",
        "bundled_runtime_fingerprint_sha256",
    ];
    if request.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err(JsonStdinError::Content);
    }
    let config = request
        .get("config")
        .and_then(Value::as_object)
        .cloned()
        .ok_or(JsonStdinError::Content)?;
    let hmac_key = request
        .get("hmac_key_hex")
        .and_then(Value::as_str)
        .and_then(decode_hmac_key)
        .ok_or(JsonStdinError::Content)?;
    let bundled_runtime_fingerprint_sha256 = match request.get("bundled_runtime_fingerprint_sha256")
    {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(JsonStdinError::Content),
    };
    Ok(BrainFingerprintRequest {
        config,
        hmac_key,
        bundled_runtime_fingerprint_sha256,
    })
}

fn decode_hmac_key(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut key = [0_u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(key)
}

fn run_brain_refresh_session(options: BrainRefreshSessionOptions) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let (expected, expect_absent) = match options.expect {
        Some(BrainRefreshExpectArg::Absent) => (None, true),
        Some(BrainRefreshExpectArg::Sha256(fingerprint)) => (Some(fingerprint), false),
        None => (None, false),
    };
    let before_revision = brain_record_revision(&line.path);
    let bundled_runtime_fingerprint_sha256 = options.bundled_runtime_fingerprint_sha256;
    let begin = solstone_core_brain::begin_refresh(
        &line.path,
        chrono::Utc::now(),
        options.run_id,
        expected.as_deref(),
        expect_absent,
        bundled_runtime_fingerprint_sha256.clone(),
    );
    let permit = match begin {
        Ok(Some(permit)) => permit,
        Ok(None)
            if brain_record_was_written(before_revision, brain_record_revision(&line.path)) =>
        {
            return write_refresh_projection(&line.path);
        }
        Ok(None) => {
            return write_session_result(json!({
                "schema": REFRESH_RESULT_SCHEMA,
                "kind": "not_started",
                "status": "no_permit",
                "reason": "lease_held",
            }));
        }
        Err(solstone_core_brain::BeginRefreshError::ExpectedFingerprintStale(error)) => {
            eprintln!("brain refresh failed: {error}");
            return ExitCode::from(EXIT_DATAERR);
        }
        Err(solstone_core_brain::BeginRefreshError::InvalidArgument(error)) => {
            eprintln!("brain refresh failed: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
        Err(solstone_core_brain::BeginRefreshError::Writer(error)) => {
            eprintln!("brain refresh failed: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    run_refresh_session_loop(
        line.path,
        permit,
        bundled_runtime_fingerprint_sha256,
        session_input_timeout(),
    )
}

fn run_brain_prerequisite_renewal_session(
    options: BrainPrerequisiteRenewalSessionOptions,
) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let bundled_runtime_fingerprint_sha256 = options.bundled_runtime_fingerprint_sha256;
    let expected_fingerprint_sha256 = options.expected_fingerprint_sha256;
    let begin = solstone_core_brain::begin_prerequisite_renewal(
        &line.path,
        chrono::Utc::now(),
        options.run_id,
        expected_fingerprint_sha256.as_deref(),
        bundled_runtime_fingerprint_sha256.clone(),
    );
    let permit = match begin {
        solstone_core_brain::BeginPrerequisiteRenewal::Started(permit) => permit,
        solstone_core_brain::BeginPrerequisiteRenewal::Busy { reason } => {
            return write_session_result(json!({
                "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
                "kind": "not_started",
                "status": "busy",
                "reason": reason,
            }));
        }
        solstone_core_brain::BeginPrerequisiteRenewal::Unsafe { reason } => {
            return write_session_result(json!({
                "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
                "kind": "not_started",
                "status": "unsafe",
                "reason": reason,
            }));
        }
    };
    run_prerequisite_renewal_session_loop(
        line.path,
        permit,
        bundled_runtime_fingerprint_sha256,
        session_input_timeout(),
    )
}

fn brain_record_revision(journal_path: &std::path::Path) -> Option<u64> {
    fs::read(solstone_core_brain::brain_state_path(journal_path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|record| record.get("revision").and_then(Value::as_u64))
}

fn brain_record_was_written(before: Option<u64>, after: Option<u64>) -> bool {
    match (before, after) {
        (None, Some(_)) => true,
        (Some(before), Some(after)) => before != after,
        _ => false,
    }
}

fn session_input_timeout() -> Duration {
    // Integration binaries are debug builds, so this bounded test hook avoids
    // making AC15 wait ninety seconds. Release builds always use the protocol
    // contract's fixed ninety-second bound.
    #[cfg(debug_assertions)]
    if let Some(timeout) = env::var("SOLSTONE_CORE_BRAIN_SESSION_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|timeout| *timeout > 0)
    {
        return Duration::from_millis(timeout);
    }
    SESSION_INPUT_TIMEOUT
}

fn run_refresh_session_loop(
    journal_path: PathBuf,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
    timeout: Duration,
) -> ExitCode {
    if write_session_result(json!({"schema": REFRESH_READY_SCHEMA})) != ExitCode::SUCCESS {
        return ExitCode::from(EXIT_IOERR);
    }
    let deadline = Instant::now() + timeout;
    match read_refresh_session_input(deadline) {
        RefreshSessionInput::Clean(outcome) => match solstone_core_brain::finish_refresh(
            &journal_path,
            permit,
            outcome,
            chrono::Utc::now(),
            bundled_runtime_fingerprint_sha256,
        ) {
            Ok(_) => write_refresh_projection(&journal_path),
            Err(error) => {
                eprintln!("brain refresh failed: {error}");
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        RefreshSessionInput::CallerAbandon(abandon) => {
            abandon_refresh_for_caller(&journal_path, permit, abandon)
        }
        RefreshSessionInput::BareEof => {
            abandon_refresh_silently(&journal_path, permit);
            // A bare EOF means no caller remains to observe an answer.
            ExitCode::from(EXIT_UNAVAILABLE)
        }
        RefreshSessionInput::Timeout => {
            abandon_refresh(&journal_path, permit, true);
            ExitCode::SUCCESS
        }
        RefreshSessionInput::ProtocolViolation => {
            abandon_refresh(&journal_path, permit, true);
            ExitCode::from(EXIT_PROTOCOL)
        }
    }
}

fn run_prerequisite_renewal_session_loop(
    journal_path: PathBuf,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
    timeout: Duration,
) -> ExitCode {
    if write_session_result(json!({"schema": PREREQUISITE_RENEWAL_READY_SCHEMA}))
        != ExitCode::SUCCESS
    {
        return ExitCode::from(EXIT_IOERR);
    }
    let deadline = Instant::now() + timeout;
    match read_prerequisite_renewal_session_input(deadline) {
        PrerequisiteRenewalSessionInput::Clean(component) => {
            match solstone_core_brain::finish_prerequisite_renewal(
                &journal_path,
                permit,
                component,
                chrono::Utc::now(),
                bundled_runtime_fingerprint_sha256,
            ) {
                Ok(_) => write_brain_projection(&journal_path, PREREQUISITE_RENEWAL_RESULT_SCHEMA),
                Err(error) => {
                    eprintln!("brain prerequisite renewal failed: {error}");
                    ExitCode::from(EXIT_UNAVAILABLE)
                }
            }
        }
        PrerequisiteRenewalSessionInput::CallerAbandon(abandon) => {
            abandon_prerequisite_renewal_for_caller(
                &journal_path,
                permit,
                abandon,
                bundled_runtime_fingerprint_sha256,
            )
        }
        PrerequisiteRenewalSessionInput::BareEof => {
            abandon_prerequisite_renewal_silently(
                &journal_path,
                permit,
                bundled_runtime_fingerprint_sha256,
            );
            // A bare EOF means no caller remains to observe an answer.
            ExitCode::from(EXIT_UNAVAILABLE)
        }
        PrerequisiteRenewalSessionInput::Timeout => {
            abandon_prerequisite_renewal(
                &journal_path,
                permit,
                bundled_runtime_fingerprint_sha256,
                true,
            );
            ExitCode::SUCCESS
        }
        PrerequisiteRenewalSessionInput::ProtocolViolation => {
            abandon_prerequisite_renewal(
                &journal_path,
                permit,
                bundled_runtime_fingerprint_sha256,
                true,
            );
            ExitCode::from(EXIT_PROTOCOL)
        }
    }
}

enum RefreshSessionInput {
    Clean(Value),
    CallerAbandon(CallerAbandon),
    BareEof,
    Timeout,
    ProtocolViolation,
}

enum PrerequisiteRenewalSessionInput {
    Clean(Value),
    CallerAbandon(CallerAbandon),
    BareEof,
    Timeout,
    ProtocolViolation,
}

struct CallerAbandon {
    reason_code: String,
    diagnostic: Map<String, Value>,
}

enum CappedLineEvent {
    Line { bytes: Vec<u8>, count: usize },
    IoError,
}

fn spawn_capped_line_reader(max_bytes: usize) -> mpsc::Receiver<CappedLineEvent> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            let mut bytes = Vec::new();
            let read = {
                let mut limited = reader.by_ref().take(max_bytes as u64);
                limited.read_until(b'\n', &mut bytes)
            };
            match read {
                Ok(count) => {
                    if sender.send(CappedLineEvent::Line { bytes, count }).is_err() || count == 0 {
                        return;
                    }
                }
                Err(_) => {
                    let _ = sender.send(CappedLineEvent::IoError);
                    return;
                }
            }
        }
    });
    receiver
}

fn read_refresh_session_input(deadline: Instant) -> RefreshSessionInput {
    let receiver = spawn_capped_line_reader(MAX_JSON_STDIN_BYTES + 1);
    let mut request = None;
    let mut terminal = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return RefreshSessionInput::Timeout;
        }
        let (line, count) = match receiver.recv_timeout(remaining) {
            Ok(CappedLineEvent::Line { bytes, count }) => (bytes, count),
            Ok(CappedLineEvent::IoError) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return RefreshSessionInput::ProtocolViolation;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return RefreshSessionInput::Timeout,
        };
        if count == 0 {
            return if terminal {
                match request.expect("terminal requires request") {
                    RefreshSessionRecord::Probe(outcome) => RefreshSessionInput::Clean(outcome),
                    RefreshSessionRecord::Abandon(abandon) => {
                        RefreshSessionInput::CallerAbandon(abandon)
                    }
                    RefreshSessionRecord::Terminal => unreachable!("terminal is not buffered"),
                }
            } else {
                RefreshSessionInput::BareEof
            };
        }
        if line.len() > MAX_JSON_STDIN_BYTES {
            return RefreshSessionInput::ProtocolViolation;
        }
        match parse_refresh_session_record(&line, chrono::Utc::now()) {
            Some(record @ (RefreshSessionRecord::Probe(_) | RefreshSessionRecord::Abandon(_)))
                if !terminal && request.is_none() =>
            {
                request = Some(record);
            }
            Some(RefreshSessionRecord::Terminal) if request.is_some() => {
                terminal = true;
            }
            _ => return RefreshSessionInput::ProtocolViolation,
        }
    }
}

fn read_prerequisite_renewal_session_input(deadline: Instant) -> PrerequisiteRenewalSessionInput {
    let receiver = spawn_capped_line_reader(MAX_JSON_STDIN_BYTES + 1);
    let mut request = None;
    let mut terminal = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return PrerequisiteRenewalSessionInput::Timeout;
        }
        let (line, count) = match receiver.recv_timeout(remaining) {
            Ok(CappedLineEvent::Line { bytes, count }) => (bytes, count),
            Ok(CappedLineEvent::IoError) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return PrerequisiteRenewalSessionInput::ProtocolViolation;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return PrerequisiteRenewalSessionInput::Timeout;
            }
        };
        if count == 0 {
            return if terminal {
                match request.expect("terminal requires request") {
                    PrerequisiteRenewalSessionRecord::Probe(component) => {
                        PrerequisiteRenewalSessionInput::Clean(component)
                    }
                    PrerequisiteRenewalSessionRecord::Abandon(abandon) => {
                        PrerequisiteRenewalSessionInput::CallerAbandon(abandon)
                    }
                    PrerequisiteRenewalSessionRecord::Terminal => {
                        unreachable!("terminal is not buffered")
                    }
                }
            } else {
                PrerequisiteRenewalSessionInput::BareEof
            };
        }
        if line.len() > MAX_JSON_STDIN_BYTES {
            return PrerequisiteRenewalSessionInput::ProtocolViolation;
        }
        match parse_prerequisite_renewal_session_record(&line, chrono::Utc::now()) {
            Some(
                record @ (PrerequisiteRenewalSessionRecord::Probe(_)
                | PrerequisiteRenewalSessionRecord::Abandon(_)),
            ) if !terminal && request.is_none() => {
                request = Some(record);
            }
            Some(PrerequisiteRenewalSessionRecord::Terminal) if request.is_some() => {
                terminal = true;
            }
            _ => return PrerequisiteRenewalSessionInput::ProtocolViolation,
        }
    }
}

enum RefreshSessionRecord {
    Probe(Value),
    Abandon(CallerAbandon),
    Terminal,
}

enum PrerequisiteRenewalSessionRecord {
    Probe(Value),
    Abandon(CallerAbandon),
    Terminal,
}

fn parse_refresh_session_record(
    bytes: &[u8],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<RefreshSessionRecord> {
    let object = serde_json::from_slice::<Value>(bytes)
        .ok()?
        .as_object()?
        .clone();
    let schema = object.get("schema")?.as_str()?;
    if schema == REFRESH_TERMINAL_SCHEMA && exact_fields(&object, &["schema"]) {
        return Some(RefreshSessionRecord::Terminal);
    }
    if schema == REFRESH_ABANDON_SCHEMA {
        return parse_caller_abandon(&object).map(RefreshSessionRecord::Abandon);
    }
    if schema != REFRESH_PROBE_SCHEMA || !exact_fields(&object, &["schema", "outcome"]) {
        return None;
    }
    let outcome = object.get("outcome")?.clone();
    solstone_core_brain::validate_refresh_probe_outcome(&outcome, now).ok()?;
    Some(RefreshSessionRecord::Probe(outcome))
}

fn parse_prerequisite_renewal_session_record(
    bytes: &[u8],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<PrerequisiteRenewalSessionRecord> {
    let object = serde_json::from_slice::<Value>(bytes)
        .ok()?
        .as_object()?
        .clone();
    let schema = object.get("schema")?.as_str()?;
    if schema == PREREQUISITE_RENEWAL_TERMINAL_SCHEMA && exact_fields(&object, &["schema"]) {
        return Some(PrerequisiteRenewalSessionRecord::Terminal);
    }
    if schema == PREREQUISITE_RENEWAL_ABANDON_SCHEMA {
        return parse_caller_abandon(&object).map(PrerequisiteRenewalSessionRecord::Abandon);
    }
    if schema != PREREQUISITE_RENEWAL_PROBE_SCHEMA
        || !exact_fields(&object, &["schema", "lane_prerequisites"])
    {
        return None;
    }
    let component = object.get("lane_prerequisites")?.clone();
    let evidence = json!({
        "configuration": null,
        "lane_prerequisites": component,
        "generate": null,
        "cogitate": null,
    });
    solstone_core_brain::validate_refresh_probe_outcome(&evidence, now).ok()?;
    Some(PrerequisiteRenewalSessionRecord::Probe(
        evidence["lane_prerequisites"].clone(),
    ))
}

fn parse_caller_abandon(object: &Map<String, Value>) -> Option<CallerAbandon> {
    if !exact_fields(object, &["schema", "reason_code"])
        && !exact_fields(object, &["schema", "reason_code", "diagnostic"])
    {
        return None;
    }
    let reason_code = object.get("reason_code")?.as_str()?;
    if reason_code.is_empty() {
        return None;
    }
    let diagnostic = match object.get("diagnostic") {
        None => Map::new(),
        Some(Value::Object(diagnostic)) => diagnostic.clone(),
        Some(_) => return None,
    };
    Some(CallerAbandon {
        reason_code: reason_code.to_owned(),
        diagnostic,
    })
}

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|field| object.contains_key(*field))
}

fn abandon_refresh_silently(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
) {
    if let Err(error) = solstone_core_brain::abandon_refresh(
        journal_path,
        permit,
        "chat_timeout",
        Map::new(),
        chrono::Utc::now(),
    ) {
        eprintln!("brain refresh abandonment failed: {error}");
    }
}

fn abandon_refresh(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    report: bool,
) {
    abandon_refresh_silently(journal_path, permit);
    if report
        && write_session_result(json!({
            "schema": REFRESH_RESULT_SCHEMA,
            "kind": "abandoned",
            "reason_code": "chat_timeout",
            "component": "generate",
        })) != ExitCode::SUCCESS
    {
        eprintln!("brain refresh abandonment report failed: stdout I/O error");
    }
}

fn abandon_refresh_for_caller(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    abandon: CallerAbandon,
) -> ExitCode {
    let before = read_brain_record_value(journal_path);
    let reason_code = abandon.reason_code;
    match solstone_core_brain::abandon_refresh(
        journal_path,
        permit,
        &reason_code,
        abandon.diagnostic,
        chrono::Utc::now(),
    ) {
        Ok(record) => match changed_evidence_component(&before, &record, &reason_code) {
            Some(component) => write_session_result(json!({
                "schema": REFRESH_RESULT_SCHEMA,
                "kind": "abandoned",
                "reason_code": reason_code,
                "component": component,
            })),
            None => {
                eprintln!("brain refresh abandonment failed: could not identify changed component");
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        Err(error) => {
            eprintln!("brain refresh abandonment failed: {error}");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}

fn write_refresh_projection(journal_path: &std::path::Path) -> ExitCode {
    write_brain_projection(journal_path, REFRESH_RESULT_SCHEMA)
}

fn abandon_prerequisite_renewal_silently(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
) {
    if let Err(error) = solstone_core_brain::abandon_prerequisite_renewal(
        journal_path,
        permit,
        "nvattest_unavailable",
        Map::new(),
        chrono::Utc::now(),
        bundled_runtime_fingerprint_sha256,
    ) {
        eprintln!("brain prerequisite renewal abandonment failed: {error}");
    }
}

fn abandon_prerequisite_renewal(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
    report: bool,
) {
    abandon_prerequisite_renewal_silently(journal_path, permit, bundled_runtime_fingerprint_sha256);
    if report
        && write_session_result(json!({
            "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
            "kind": "abandoned",
            "reason_code": "nvattest_unavailable",
            "component": "lane_prerequisites",
        })) != ExitCode::SUCCESS
    {
        eprintln!("brain prerequisite renewal abandonment report failed: stdout I/O error");
    }
}

fn abandon_prerequisite_renewal_for_caller(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    abandon: CallerAbandon,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> ExitCode {
    let before = read_brain_record_value(journal_path);
    let reason_code = abandon.reason_code;
    match solstone_core_brain::abandon_prerequisite_renewal(
        journal_path,
        permit,
        &reason_code,
        abandon.diagnostic,
        chrono::Utc::now(),
        bundled_runtime_fingerprint_sha256,
    ) {
        Ok(record) => match changed_evidence_component(&before, &record, &reason_code) {
            Some(component) => write_session_result(json!({
                "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
                "kind": "abandoned",
                "reason_code": reason_code,
                "component": component,
            })),
            None => {
                eprintln!(
                    "brain prerequisite renewal abandonment failed: could not identify changed component"
                );
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        Err(error) => {
            eprintln!("brain prerequisite renewal abandonment failed: {error}");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}

fn read_brain_record_value(journal_path: &std::path::Path) -> Option<Value> {
    fs::read(solstone_core_brain::brain_state_path(journal_path))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn changed_evidence_component(
    before: &Option<Value>,
    after: &Value,
    reason_code: &str,
) -> Option<String> {
    let before_evidence = before
        .as_ref()
        .and_then(|record| record.get("evidence"))
        .and_then(Value::as_object);
    after
        .get("evidence")
        .and_then(Value::as_object)?
        .iter()
        .find_map(|(component, value)| {
            (before_evidence.and_then(|evidence| evidence.get(component)) != Some(value)
                && value.get("reason_code").and_then(Value::as_str) == Some(reason_code))
            .then(|| component.clone())
        })
}

fn write_brain_projection(journal_path: &std::path::Path, result_schema: &str) -> ExitCode {
    let config = match read_journal_config(journal_path) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            eprintln!("brain refresh failed: could not read journal config: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    let projection =
        solstone_core_brain::inspect_brain_state(journal_path, &config, chrono::Utc::now())
            .projection;
    write_session_result(json!({
        "schema": result_schema,
        "kind": "projection",
        "projection": {
            "aggregate_state": projection.aggregate_state,
            "reason_code": projection.reason_code,
            "active_lane": projection.active_lane,
            "active_provider": projection.active_provider,
            "active_model": projection.active_model,
            "fingerprint_sha256": projection.fingerprint_sha256,
            "runtime_transition_in_progress": projection.runtime_transition_in_progress,
        },
    }))
}

fn write_session_result(output: Value) -> ExitCode {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        let written = serde_json::to_writer(&mut stdout, &output).is_ok()
            && stdout.write_all(b"\n").is_ok()
            && stdout.flush().is_ok();
        let _ = sender.send(written);
    });
    if receiver
        .recv_timeout(SESSION_RESULT_WRITE_TIMEOUT)
        .is_ok_and(|written| written)
    {
        ExitCode::SUCCESS
    } else {
        eprintln!("brain refresh failed: stdout I/O error");
        ExitCode::from(EXIT_IOERR)
    }
}

struct RuntimeFailureRequest {
    reason_code: String,
    component: String,
    expected_fingerprint_sha256: String,
    diagnostic: Map<String, Value>,
    bundled_runtime_fingerprint_sha256: Option<String>,
}

fn run_brain_runtime_failure(options: BrainRuntimeFailureOptions) -> ExitCode {
    let request = match read_bounded_json_stdin().and_then(parse_runtime_failure_request) {
        Ok(request) => request,
        Err(JsonStdinError::Content) => {
            eprintln!(
                "brain record-runtime-failure failed: stdin was not a valid request JSON object within 1 MiB"
            );
            return ExitCode::from(EXIT_USAGE);
        }
        Err(JsonStdinError::Io) => {
            eprintln!("brain record-runtime-failure failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let result = solstone_core_brain::record_runtime_failure(
        &line.path,
        &request.reason_code,
        &request.component,
        &request.expected_fingerprint_sha256,
        request.diagnostic,
        chrono::Utc::now(),
        request.bundled_runtime_fingerprint_sha256,
    );
    let output = json!({
        "accepted": result.accepted,
        "record": result.record,
        "rejected_reason": result.rejected_reason,
        "error": result.error,
    });
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err() || stdout.write_all(b"\n").is_err() {
        eprintln!("brain record-runtime-failure failed: stdout I/O error");
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

fn parse_runtime_failure_request(
    request: Map<String, Value>,
) -> Result<RuntimeFailureRequest, JsonStdinError> {
    const FIELDS: [&str; 5] = [
        "reason_code",
        "component",
        "expected_fingerprint_sha256",
        "diagnostic",
        "bundled_runtime_fingerprint_sha256",
    ];
    if request.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err(JsonStdinError::Content);
    }
    let string = |field: &str| {
        request
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or(JsonStdinError::Content)
    };
    let diagnostic = match request.get("diagnostic") {
        None => Map::new(),
        Some(Value::Object(value)) => value.clone(),
        Some(_) => return Err(JsonStdinError::Content),
    };
    let bundled_runtime_fingerprint_sha256 = match request.get("bundled_runtime_fingerprint_sha256")
    {
        None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) => return Err(JsonStdinError::Content),
    };
    Ok(RuntimeFailureRequest {
        reason_code: string("reason_code")?,
        component: string("component")?,
        expected_fingerprint_sha256: string("expected_fingerprint_sha256")?,
        diagnostic,
        bundled_runtime_fingerprint_sha256,
    })
}

fn run_journal_config_read(options: JournalConfigReadOptions) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let read = match read_journal_config(&line.path) {
        Ok(read) => read,
        Err(error) => {
            eprintln!("journal-config read failed: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    let config = read.config.unwrap_or_else(materialized_defaults);
    let output = json!({
        "present": read.present,
        "sha256": read.sha256,
        "config": config,
    });
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err() || stdout.write_all(b"\n").is_err() {
        eprintln!("journal-config read failed: stdout I/O error");
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

fn run_journal_config_commit(options: JournalConfigCommitOptions) -> ExitCode {
    let replacement = match read_journal_config_stdin() {
        Ok(replacement) => replacement,
        Err(JsonStdinError::Content) => {
            eprintln!(
                "journal-config commit failed: stdin was not a valid JSON object within 1 MiB"
            );
            return ExitCode::from(EXIT_USAGE);
        }
        Err(JsonStdinError::Io) => {
            eprintln!("journal-config commit failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let expectation = match options.expect {
        JournalConfigExpectArg::Absent => ConfigExpectation::Absent,
        JournalConfigExpectArg::Sha256(fingerprint) => ConfigExpectation::Sha256(fingerprint),
    };
    let lock_options = LockOptions {
        timeout: options
            .lock_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| LockOptions::default().timeout),
        ..LockOptions::default()
    };
    match commit_journal_config(&line.path, expectation, &replacement, lock_options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let exit = commit_config_error_exit(&error);
            eprintln!("journal-config commit failed: {error}");
            ExitCode::from(exit)
        }
    }
}

fn resolve_journal_config_path(
    journal_override: Option<std::ffi::OsString>,
) -> Result<JournalPathLine, JournalPathError> {
    match journal_override {
        Some(path) => Ok(JournalPathLine {
            label: "cli",
            path: PathBuf::from(path),
        }),
        None => resolve_process_journal_path(),
    }
}

enum JsonStdinError {
    Content,
    Io,
}

fn read_journal_config_stdin() -> Result<Map<String, Value>, JsonStdinError> {
    read_bounded_json_stdin()
}

fn read_bounded_json_stdin() -> Result<Map<String, Value>, JsonStdinError> {
    let value = read_bounded_json_value(MAX_JSON_STDIN_BYTES)?;
    value.as_object().cloned().ok_or(JsonStdinError::Content)
}

fn read_bounded_json_value(max_bytes: usize) -> Result<Value, JsonStdinError> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin.read(&mut chunk).map_err(|_| JsonStdinError::Io)?;
        if read == 0 {
            break;
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|next| next > max_bytes)
        {
            return Err(JsonStdinError::Content);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    serde_json::from_slice::<Value>(&bytes).map_err(|_| JsonStdinError::Content)
}

fn commit_config_error_exit(error: &CommitConfigError) -> u8 {
    match error {
        CommitConfigError::Conflict(_) => EXIT_DATAERR,
        CommitConfigError::Load(_) => EXIT_UNAVAILABLE,
        CommitConfigError::Write(_) => EXIT_CANTCREAT,
        CommitConfigError::Lock(LockError::Io { .. }) => EXIT_IOERR,
        CommitConfigError::Lock(LockError::Timeout(_)) => EXIT_TEMPFAIL,
    }
}

fn run_spl_process(command: SplCommand) -> ExitCode {
    match command {
        SplCommand::Service(options) => run_spl_service(options),
    }
}

fn run_spl_service(options: ServiceOptions) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let verbosity = solstone_core_spl::Verbosity::from_flags(options.verbose, options.debug);
    match solstone_core_spl::run_native_service(journal.path, verbosity) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spl service failed: {}", error.class());
            ExitCode::from(EXIT_TEMPFAIL)
        }
    }
}

fn run_journal_path(options: JournalPathOptions) -> Result<JournalPathLine, JournalPathError> {
    let line = if let Some(path) = options.journal_override {
        JournalPathLine {
            label: "cli",
            path: PathBuf::from(path),
        }
    } else {
        resolve_process_journal_path()?
    };

    if options.create {
        ensure_journal_dir_with_label(&line.path, line.label).map_err(JournalPathError::Create)?;
    }

    Ok(line)
}

fn run_body(command: BodyCommand) -> ExitCode {
    match command {
        BodyCommand::Rebuild(options) => run_body_rebuild(options),
        BodyCommand::Apple(options) => run_body_apple(options),
        BodyCommand::Oura(command) => run_body_oura(command),
    }
}

fn run_body_oura(command: BodyOuraCommand) -> ExitCode {
    match command {
        BodyOuraCommand::Connect(options) => run_body_oura_connect(options),
        BodyOuraCommand::Sync(options) => run_body_oura_sync(options),
    }
}

fn run_body_oura_connect(options: BodyOuraConnectOptions) -> ExitCode {
    use solstone_core_body_ingest::{BodyIngestErrorKind, OuraConnectOptions, connect_oura};

    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match connect_oura(&journal, &OuraConnectOptions::default()) {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "schema": "solstone.body.oura.connect.result.v1",
                    "connected": true,
                    "scopes": report.scopes(),
                }));
            } else {
                println!("Oura authorization saved to journal config.");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Oura authorization failed: {error}");
            ExitCode::from(match error.kind() {
                BodyIngestErrorKind::Gate => 2,
                BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
            })
        }
    }
}

fn run_body_oura_sync(options: BodyOuraSyncOptions) -> ExitCode {
    use solstone_core_body_ingest::{BodyIngestErrorKind, OuraSyncOptions, sync_oura};

    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let result = sync_oura(
        &journal,
        &OuraSyncOptions {
            save: options.save,
            confirm_body_save: options.confirm_body_save,
            scheduled: options.scheduled,
            window_days: options.window_days,
            today: None,
        },
    );
    match result {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "schema": "solstone.body.ingest.result.v1",
                    "source": "oura_api",
                    "mode": if options.save { "save" } else { "preview" },
                    "bundle_id": report.bundle_id(),
                    "rows": report.rows(),
                    "days": report.days(),
                    "pages": report.pages(),
                    "skipped": report.quiet_run(),
                    "endpoint_counts": report.endpoint_counts(),
                    "issues": report.issues().iter().map(|issue| json!({
                        "endpoint": issue.endpoint(),
                        "kind": issue.kind().as_str(),
                    })).collect::<Vec<_>>(),
                }));
            } else if options.save {
                println!(
                    "Oura body sync complete: rows={} days={} pages={} bundle={}",
                    report.rows(),
                    report.days().len(),
                    report.pages(),
                    report.bundle_id().unwrap_or("none")
                );
            } else {
                println!(
                    "Oura body sync preview: rows={} days={} pages={} (nothing written)",
                    report.rows(),
                    report.days().len(),
                    report.pages()
                );
            }
            if !options.json {
                for issue in report.issues() {
                    println!(
                        "Oura endpoint unavailable: {} ({})",
                        issue.endpoint(),
                        issue.kind().as_str()
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Oura body sync failed: {error}");
            ExitCode::from(match error.kind() {
                BodyIngestErrorKind::Gate => 2,
                BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
            })
        }
    }
}

fn run_body_apple(options: BodyAppleOptions) -> ExitCode {
    use solstone_core_body_ingest::{
        AppleImportOptions, BodyIngestErrorKind, detect_apple_source, preview_apple, save_apple,
    };

    let source = PathBuf::from(options.source);
    if options.detect {
        return match detect_apple_source(&source) {
            Ok(detected) => {
                if options.json {
                    print_json(&json!({
                        "schema": "solstone.body.apple.detect.result.v1",
                        "apple_health": detected,
                    }));
                } else {
                    println!(
                        "{}",
                        if detected {
                            "apple_health"
                        } else {
                            "not_apple_health"
                        }
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Apple Health source detection failed: {error}");
                ExitCode::from(match error.kind() {
                    BodyIngestErrorKind::Gate => 2,
                    BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                    BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
                })
            }
        };
    }
    let result = if options.save {
        let journal = match resolve_indexer_journal_path(options.journal_override) {
            Ok(line) => line.path,
            Err(error) => return print_journal_error(error),
        };
        save_apple(
            &source,
            &journal,
            &AppleImportOptions {
                date_from: options.date_from,
                date_to: options.date_to,
                confirm_body_save: options.confirm_body_save,
                force: options.force,
            },
        )
    } else {
        preview_apple(
            &source,
            options.date_from.as_deref(),
            options.date_to.as_deref(),
        )
    };
    match result {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "schema": "solstone.body.ingest.result.v1",
                    "source": "apple_health",
                    "mode": if options.save { "save" } else { "preview" },
                    "bundle_id": report.bundle_id(),
                    "rows": report.rows(),
                    "days": report.days(),
                    "skipped": report.skipped(),
                }));
            } else if options.save {
                println!(
                    "Apple Health body import complete: rows={} days={} bundle={}",
                    report.rows(),
                    report.days().len(),
                    report.bundle_id().unwrap_or("none")
                );
            } else {
                println!(
                    "Apple Health body import preview: rows={} days={} (nothing written)",
                    report.rows(),
                    report.days().len()
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Apple Health body import failed: {error}");
            ExitCode::from(match error.kind() {
                BodyIngestErrorKind::Gate => 2,
                BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
            })
        }
    }
}

fn run_body_rebuild(options: BodyRebuildOptions) -> ExitCode {
    let json_output = options.json;
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match solstone_core_body_rebuild::rebuild_body_store(&journal) {
        Ok(report) => {
            if json_output {
                print_json(&json!({
                    "schema": "solstone.body.rebuild.result.v1",
                    "native_bundles": report.native_bundles(),
                    "legacy_bundles": report.legacy_bundles(),
                    "rows": report.rows(),
                }));
            } else {
                println!(
                    "body rebuild complete: native_bundles={} legacy_bundles={} rows={}",
                    report.native_bundles(),
                    report.legacy_bundles(),
                    report.rows()
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("body rebuild failed: {error}");
            let exit = body_rebuild_error_exit(error.kind(), error.stage());
            ExitCode::from(exit)
        }
    }
}

fn body_rebuild_error_exit(
    kind: solstone_core_body_rebuild::BodyRebuildErrorKind,
    stage: &str,
) -> u8 {
    use solstone_core_body_rebuild::BodyRebuildErrorKind;

    match (kind, stage) {
        (BodyRebuildErrorKind::Publication, "database_lock_timeout") => EXIT_TEMPFAIL,
        (
            BodyRebuildErrorKind::Journal
            | BodyRebuildErrorKind::Sqlite
            | BodyRebuildErrorKind::Publication,
            _,
        ) => EXIT_IOERR,
        _ => EXIT_DATAERR,
    }
}

fn run_indexer(command: IndexerCommand) -> ExitCode {
    match command {
        IndexerCommand::Maintenance(options) => run_indexer_maintenance(options),
        IndexerCommand::Search(options) => run_indexer_search(options),
        IndexerCommand::Counts(options) => run_indexer_counts(options),
        IndexerCommand::Agents(options) => run_indexer_agents(options),
        IndexerCommand::Coverage(options) => run_indexer_coverage(options),
        IndexerCommand::PruneStream(options) => run_indexer_prune_stream(options),
        IndexerCommand::PrunePaths(options) => run_indexer_prune_paths(options),
        IndexerCommand::FoldEntityEdges(options) => run_indexer_fold_entity_edges(options),
        IndexerCommand::EdgeFingerprint(options) => run_indexer_edge_fingerprint(options),
        IndexerCommand::RebuildEdgesFingerprint(options) => {
            run_indexer_rebuild_edges_fingerprint(options)
        }
    }
}

fn run_indexer_maintenance(options: IndexerOptions) -> ExitCode {
    let json_output = options.json;
    if !options.reset
        && !options.rebuild_edges
        && !options.rescan
        && !options.rescan_full
        && options.rescan_file.is_none()
    {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let line = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };

    if options.reset
        && let Err(error) = reset_index(&line.path)
    {
        eprintln!("indexer reset failed: {error}");
        return ExitCode::from(EXIT_TEMPFAIL);
    }

    if options.rebuild_edges {
        match rebuild_edges(&line.path) {
            Ok(report) => {
                for warning in report.warnings {
                    eprintln!("warning: {warning}");
                }
                if report.failed > 0 {
                    return ExitCode::from(EXIT_TEMPFAIL);
                }
                if json_output && !options.rescan && !options.rescan_full {
                    print_json(&json!({
                        "files": report.files,
                        "rows": report.rows,
                        "drops": report.drops,
                        "failed": report.failed,
                        "skipped": report.skipped,
                    }));
                }
            }
            Err(error) => {
                eprintln!("indexer edge rebuild failed: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    }

    if let Some(path) = options.rescan_file {
        match rescan_file(&line.path, &PathBuf::from(path)) {
            Ok(RescanFileStatus::Indexed { warnings }) => {
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
                if json_output {
                    print_json(&json!({"indexed": true}));
                }
                return ExitCode::SUCCESS;
            }
            Ok(RescanFileStatus::Declined) => {
                eprintln!("indexer declined unsupported file");
                return ExitCode::from(EXIT_UNAVAILABLE);
            }
            Err(error) => {
                eprintln!("indexer rescan-file failed: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    }

    if options.rescan || options.rescan_full {
        match scan_journal(&line.path, options.rescan_full) {
            Ok(report) => {
                for warning in report.warnings {
                    eprintln!("warning: {warning}");
                }
                let should_emit_zero_edge_hint = options.rescan_full
                    && !options.rebuild_edges
                    && !options.reset
                    && report.edge_rows_inserted == 0;
                if should_emit_zero_edge_hint && !json_output {
                    println!("{ZERO_EDGE_HINT}");
                }
                if json_output {
                    print_json(&json!({
                        "indexed": report.indexed,
                        "removed": report.removed,
                        "skipped": report.skipped,
                        "edges_indexed": report.edges_indexed,
                        "edges_removed": report.edges_removed,
                        "edge_rows_inserted": report.edge_rows_inserted,
                        "merge_steps_spent": report.merge_steps_spent,
                        "failed": report.failed,
                    }));
                }
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("indexer scan failed: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    }

    if json_output {
        print_json(&json!({"reset": options.reset}));
    }
    ExitCode::SUCCESS
}

fn run_indexer_prune_stream(options: IndexerPruneStreamOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match prune_chunks_by_stream(&journal, &options.stream) {
        Ok(counts) => {
            if options.json {
                print_json(&json!({"chunks": counts.chunks, "files": counts.files}));
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("prune-stream", error),
    }
}

fn run_indexer_prune_paths(options: IndexerPrunePathsOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let paths = options.paths.iter().map(String::as_str).collect::<Vec<_>>();
    match prune_by_paths(&journal, &paths) {
        Ok(counts) => {
            let counts = counts.unwrap_or_default();
            if options.json {
                print_json(&json!({"chunks": counts.chunks, "files": counts.files}));
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("prune-paths", error),
    }
}

fn run_indexer_fold_entity_edges(options: IndexerFoldEntityEdgesOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match fold_entity_edges_for_recorded_merge(&journal, &options.source_id, &options.target_id) {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "rows_folded": report.rows_folded,
                    "self_edges_dropped": report.self_edges_dropped,
                    "fallback_rebuild": report.fallback_rebuild,
                }));
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("fold-entity-edges", error),
    }
}

fn run_indexer_edge_fingerprint(options: IndexerReadOptions) -> ExitCode {
    run_indexer_fingerprint(options, false)
}

fn run_indexer_rebuild_edges_fingerprint(options: IndexerReadOptions) -> ExitCode {
    run_indexer_fingerprint(options, true)
}

fn run_indexer_fingerprint(options: IndexerReadOptions, rebuild: bool) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let result = if rebuild {
        rebuild_edges_for_recorded_merge_undo(&journal)
    } else {
        fingerprint_edge_rows(&journal)
    };
    match result {
        Ok(fingerprint) => {
            if options.json {
                print_json(&json!({"fingerprint": fingerprint}));
            } else {
                println!("{fingerprint}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("edge-fingerprint", error),
    }
}

fn print_indexer_mutation_error(
    operation: &str,
    error: solstone_core_indexer_store::StoreError,
) -> ExitCode {
    eprintln!("indexer {operation} failed: {error}");
    ExitCode::from(EXIT_TEMPFAIL)
}

fn run_indexer_search(options: IndexerSearchOptions) -> ExitCode {
    let json = options.query.json;
    let request = match search_request(
        &options.query,
        options.limit,
        options.offset,
        options.counts,
        &options.order,
    ) {
        Ok(request) => request,
        Err(()) => return print_usage_error(),
    };
    let journal = match resolve_indexer_journal_path(options.query.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match search(&journal, &request, Local::now().date_naive()) {
        Ok(response) => {
            if json {
                print_json(&response);
            } else {
                println!("{} result(s)", response.results.len());
                for hit in response.results {
                    println!("{}\t{}", hit.id, hit.text);
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, json),
    }
}

fn run_indexer_counts(options: IndexerCountsOptions) -> ExitCode {
    let json = options.query.json;
    let request = match search_request(&options.query, 10, 0, false, "relevance") {
        Ok(request) => request,
        Err(()) => return print_usage_error(),
    };
    let journal = match resolve_indexer_journal_path(options.query.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match search_counts(&journal, &request, Local::now().date_naive()) {
        Ok(response) => {
            if json {
                print_json(&response);
            } else {
                println!("{} matching chunk(s)", response.total);
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, json),
    }
}

fn run_indexer_agents(options: IndexerReadOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match agents(&journal) {
        Ok(response) => {
            if options.json {
                print_json(&response);
            } else {
                for agent in response {
                    println!("{agent}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, options.json),
    }
}

fn run_indexer_coverage(options: IndexerReadOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match coverage(&journal) {
        Ok(response) => {
            if options.json {
                print_json(&response);
            } else if let (Some(start), Some(end)) = (response.start, response.end) {
                println!("available: {start} through {end}");
            } else {
                println!("no dated chunks");
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, options.json),
    }
}

fn search_request(
    options: &IndexerQueryOptions,
    limit: usize,
    offset: usize,
    counts: bool,
    order: &str,
) -> Result<SearchRequest, ()> {
    let order = match order {
        "relevance" => Order::Relevance,
        "recency" => Order::Recency,
        "reranked" => Order::Reranked,
        _ => return Err(()),
    };
    let mut request =
        SearchRequest::new(options.query.clone().unwrap_or_default(), order).map_err(|_| ())?;
    request.limit = limit;
    request.offset = offset;
    request.day = options.day.clone();
    request.day_from = options.day_from.clone();
    request.day_to = options.day_to.clone();
    request.facet = options.facet.clone();
    request.agent = options.agent.clone();
    request.stream = options.stream.clone();
    request.time_bucket = options.time_bucket.clone();
    request.relax = options.relax;
    request.counts = counts;
    Ok(request)
}

fn resolve_indexer_journal_path(
    journal_override: Option<std::ffi::OsString>,
) -> Result<JournalPathLine, JournalPathError> {
    if let Some(path) = journal_override {
        Ok(JournalPathLine {
            label: "cli",
            path: PathBuf::from(path),
        })
    } else {
        resolve_process_journal_path()
    }
}

fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string(value).expect("query responses serialize to JSON")
    );
}

fn print_index_access_error(error: IndexAccessError, json: bool) -> ExitCode {
    if json {
        print_json(&serde_json::json!({
            "error": {"reason": error.reason(), "message": error.to_string()}
        }));
    } else {
        eprintln!("Error: {error}");
    }
    let exit = if error.reason() == "index_locked" {
        EXIT_TEMPFAIL
    } else {
        EXIT_UNAVAILABLE
    };
    ExitCode::from(exit)
}

fn print_usage_error() -> ExitCode {
    eprint!("{USAGE}");
    ExitCode::from(EXIT_USAGE)
}

fn print_journal_error(error: JournalPathError) -> ExitCode {
    eprint_journal_path_error(error);
    ExitCode::from(EXIT_TEMPFAIL)
}

fn resolve_process_journal_path() -> Result<JournalPathLine, JournalPathError> {
    let env_journal = env::var_os("SOLSTONE_JOURNAL");
    if let Some(path) = env_journal
        .as_deref()
        .filter(|value| *value != OsStr::new(""))
    {
        return Ok(JournalPathLine {
            label: "env",
            path: PathBuf::from(path),
        });
    }

    let home = discover_binary_home().map_err(JournalPathError::Home)?;
    let config_journal = read_config_journal(&home).map_err(JournalPathError::Config)?;
    let resolved = resolve_journal_path(
        env_journal.as_deref(),
        config_journal.as_deref(),
        None,
        &home,
    );
    Ok(JournalPathLine {
        label: match resolved.source {
            Source::Env => "env",
            Source::Config => "config",
            Source::Source => "source",
            Source::Default => "default",
        },
        path: resolved.path,
    })
}

fn discover_binary_home() -> Result<PathBuf, HomeError> {
    let home_env = env::var_os("HOME");
    if let Some(home) = home_env.as_deref() {
        return discover_home(Some(home), None);
    }
    let fallback = env::home_dir();
    discover_home(None, fallback.as_deref())
}

fn eprint_journal_path_error(error: JournalPathError) {
    match error {
        JournalPathError::Config(ConfigError::Decode) => {
            eprintln!("journal-path failed: config is not valid UTF-8")
        }
        JournalPathError::Home(HomeError::Unavailable) => {
            eprintln!("journal-path failed: could not determine home directory")
        }
        JournalPathError::Create(error) => eprintln!("{error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    use solstone_core_journal_config_write::{
        AtomicWriteError, ConfigConflict, ConfigExpectation, ConfigFingerprint, LockTimeout,
    };

    use super::*;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_path() -> PathBuf {
        PathBuf::from("/tmp/journal-config-test")
    }

    #[test]
    fn exit_code_constants_match_the_documented_contract() {
        assert_eq!(EXIT_USAGE, 64);
        assert_eq!(EXIT_DATAERR, 65);
        assert_eq!(EXIT_UNAVAILABLE, 69);
        assert_eq!(EXIT_CANTCREAT, 73);
        assert_eq!(EXIT_IOERR, 74);
        assert_eq!(EXIT_TEMPFAIL, 75);
        assert_eq!(EXIT_PROTOCOL, 76);
    }

    #[test]
    fn body_rebuild_errors_distinguish_data_io_and_retryable_lock_failure() {
        use solstone_core_body_rebuild::BodyRebuildErrorKind;

        assert_eq!(
            body_rebuild_error_exit(BodyRebuildErrorKind::Publication, "database_lock_timeout"),
            EXIT_TEMPFAIL
        );
        for (kind, stage) in [
            (BodyRebuildErrorKind::Journal, "journal_root"),
            (BodyRebuildErrorKind::Sqlite, "open_temp_database"),
            (BodyRebuildErrorKind::Publication, "database_lock"),
        ] {
            assert_eq!(body_rebuild_error_exit(kind, stage), EXIT_IOERR);
        }
        for kind in [
            BodyRebuildErrorKind::Authority,
            BodyRebuildErrorKind::Envelope,
            BodyRebuildErrorKind::NativeReplay,
            BodyRebuildErrorKind::LegacyReplay,
        ] {
            assert_eq!(body_rebuild_error_exit(kind, "semantic"), EXIT_DATAERR);
        }
    }

    #[test]
    fn generate_response_write_failure_is_reportable_as_internal_failure() {
        let mut writer = FailingWriter;
        assert!(write_generate_response_to(&mut writer, "{}").is_err());
    }

    #[test]
    fn brain_record_write_observation_requires_a_new_revision() {
        assert!(!brain_record_was_written(None, None));
        assert!(!brain_record_was_written(Some(7), Some(7)));
        assert!(brain_record_was_written(None, Some(1)));
        assert!(brain_record_was_written(Some(7), Some(8)));
    }

    #[test]
    fn commit_config_error_exit_maps_all_variants() {
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Conflict(ConfigConflict {
                expected: ConfigExpectation::Absent,
                actual: ConfigFingerprint::Absent,
            })),
            EXIT_DATAERR
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Load(
                solstone_core_journal_config::ConfigLoadError::Corrupt {
                    path: test_path(),
                    source: Box::new(io::Error::other("test")),
                },
            )),
            EXIT_UNAVAILABLE
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Write(AtomicWriteError::Io {
                path: test_path(),
                source: io::Error::other("test"),
            })),
            EXIT_CANTCREAT
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Lock(LockError::Io {
                path: test_path(),
                source: io::Error::other("test"),
            })),
            EXIT_IOERR
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Lock(LockError::Timeout(LockTimeout {
                path: test_path(),
                timeout: Duration::from_millis(1),
            },))),
            EXIT_TEMPFAIL
        );
    }

    #[test]
    fn anthropic_generated_response_preserves_thinking_blocks() {
        let request = solstone_core_generate::GenerateRequest {
            id: Some("request".into()),
            context: "context".into(),
            contents: Vec::new(),
            system_instruction: None,
            temperature: 0.0,
            max_output_tokens: 1,
            thinking_budget: None,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        };
        let response = anthropic_generated_response(
            &request,
            solstone_core_generate_wire::AnthropicGenerated {
                text: "text".into(),
                model: "model".into(),
                usage: json!({"input_tokens": 1}),
                finish_reason: "stop".into(),
                thinking: Some(json!({"type": "thinking", "thinking": "work"})),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            response.thinking,
            Some(json!({"type": "thinking", "thinking": "work"}))
        );
    }
}
