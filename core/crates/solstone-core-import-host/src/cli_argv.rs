// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal importer argv parsing and dispatch.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use ffmpeg_next as ffmpeg;
use serde_json::Value;
use serde_json::json;
use solstone_core_segment::{
    SUPERVISOR_MESSAGE, SupervisorRefusal, require_solstone, require_solstone_with,
};

use solstone_core_import::cli_render::{self, CliRun};
use solstone_core_import::connect::{OuraConnectRequest, connect_oura};
use solstone_core_import::contract::{AudioAuto, SyncPreviewRequest, SyncSaveRequest};
use solstone_core_import::detect::{
    ManifestSummary, RegistrySource, ResolutionOptions, ResolutionOutcome, ResolutionSeams,
    ResolvedSource, resolve_import,
};
use solstone_core_import::publish::{PublicationInput, PublicationStatus, publish};
use solstone_core_import::sync_audio::{
    AudioCandidate, AudioPreviewSeams, AudioProbe, AudioSaveSeams, AudioSyncRequest,
    DirectoryScanner, FilesystemAudioStateWriter, ManifestLookup, sync_audio_preview,
    sync_audio_save,
};
use solstone_core_import::sync_obsidian::{
    ObsidianHomeCandidates, ObsidianNote, ObsidianPreviewSeams, ObsidianSaveSeams, ObsidianScanner,
    ObsidianSyncRequest, ObsidianWriter, sync_obsidian_preview, sync_obsidian_save,
};
use solstone_core_import::sync_plaud::{
    FilesystemPlaudStateWriter, ImportPipeline, PipelineAuto, PipelineImportRequest,
    PipelineOutcome, PlaudCatalogue, PlaudCredential, PlaudDownload, PlaudFailureKind, PlaudFile,
    PlaudManifestLookup, PlaudPreviewSeams, PlaudSaveSeams, PlaudSyncRequest, SyncClock,
    sanitize_filename, sync_plaud_preview, sync_plaud_save,
};
use solstone_core_import_sources::{obsidian, save};

use crate::audio::{AudioImportRequest, import_audio};
use crate::audio_publication::finish_audio_attempt;
use crate::import_publication::{ImportTerminalInput, finish_import_attempt};
use solstone_core_import::metadata::{admit_running_attempt, refuse_if_live_running};
use solstone_core_import::text::TextImportOutcome;

/// Result of parsing and resolving one importer invocation.
#[derive(Debug, Eq, PartialEq)]
pub enum CliOutcome {
    /// The import crate fully handled this invocation.
    Rendered(CliRun),
    /// The top-level binary must invoke the source-specific body.
    Registry(RegistryDispatch),
}

/// Source-body inputs that cross from the import grammar to the owning binary.
#[derive(Debug, Eq, PartialEq)]
pub struct RegistryDispatch {
    pub media: PathBuf,
    pub source: RegistrySource,
    pub timestamp: String,
    pub dry_run: bool,
    pub force: bool,
}

/// Run the importer grammar with the process environment and local supervisor probe.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliOutcome {
    let parsed = match parse_arguments(args) {
        Ok(ParsedCommand::Help) => return rendered(success(cli_render::HELP.to_owned())),
        Ok(parsed) => parsed,
        Err(arguments) => return rendered(argparse_error(arguments)),
    };
    run_after_parse(parsed, journal_path, || require_solstone(journal_path))
}

/// Run the importer grammar with injectable environment and supervisor seams.
pub fn run_cli_with<E, C>(
    args: &[String],
    journal_path: &Path,
    lookup_env: E,
    connectivity: C,
) -> CliOutcome
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    let parsed = match parse_arguments(args) {
        Ok(ParsedCommand::Help) => return rendered(success(cli_render::HELP.to_owned())),
        Ok(parsed) => parsed,
        Err(arguments) => return rendered(argparse_error(arguments)),
    };
    run_after_parse(parsed, journal_path, || {
        require_solstone_with(lookup_env, connectivity)
    })
}

fn run_after_parse(
    parsed: ParsedCommand,
    journal_path: &Path,
    preflight: impl FnOnce() -> Result<(), SupervisorRefusal>,
) -> CliOutcome {
    match preflight() {
        Ok(()) => {}
        Err(SupervisorRefusal::SpawnedUnavailable) => return rendered(failure("", "", 75)),
        Err(SupervisorRefusal::Unavailable) => {
            return rendered(failure("", &format!("{SUPERVISOR_MESSAGE}\n"), 1));
        }
    }

    match parsed {
        ParsedCommand::Help => unreachable!("help returns before supervisor preflight"),
        ParsedCommand::ListImporters { json } => rendered(success(cli_render::importers(json))),
        ParsedCommand::Backends => rendered(success(cli_render::backends())),
        ParsedCommand::Connect { backend } => rendered(run_connect(&backend, journal_path)),
        ParsedCommand::Sync { backend, options } => {
            rendered(run_sync(&backend, &options, journal_path))
        }
        ParsedCommand::Import(options) => run_import(options, journal_path),
    }
}

fn run_connect(backend: &str, journal_path: &Path) -> CliRun {
    if backend != "oura" {
        return failure(
            "",
            &format!("Unknown connect backend: {backend}\nConnectable backends: oura\n"),
            1,
        );
    }
    match connect_oura(&OuraConnectRequest {
        journal_root: journal_path.to_path_buf(),
        timeout_seconds: 300,
    }) {
        Ok(outcome) => success(format!(
            "Oura authorization saved to journal config.\nAuthorized scopes: {}\n",
            outcome.report.scopes().join(" ")
        )),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn run_sync(backend: &str, options: &Options, journal_path: &Path) -> CliRun {
    match backend {
        "oura" => {
            let result = solstone_core_body_ingest::sync_oura(
                journal_path,
                &solstone_core_body_ingest::OuraSyncOptions {
                    save: options.save,
                    confirm_body_save: options.confirm_body_save,
                    scheduled: options.scheduled,
                    window_days: options.window_days,
                    today: None,
                },
            );
            match result {
                Ok(report) => success(format!(
                    "Oura body sync {}: rows={} days={} pages={}\n",
                    if options.save { "complete" } else { "preview" },
                    report.rows(),
                    report.days().len(),
                    report.pages()
                )),
                Err(error) => failure("", &format!("{error}\n"), 1),
            }
        }
        "plaud" => run_plaud_sync(journal_path, options),
        "obsidian" => run_obsidian_sync(journal_path, options),
        "audio" => run_audio_sync(journal_path, options),
        _ => failure(
            "",
            &format!(
                "Unknown sync backend: {backend}\nAvailable backends: plaud, obsidian, audio, oura\n"
            ),
            1,
        ),
    }
}

fn run_import(options: Options, journal_path: &Path) -> CliOutcome {
    let Some(media) = options.media.as_deref() else {
        return rendered(argparse_error(
            "the following arguments are required: media".to_owned(),
        ));
    };
    let outcome = match resolve(options_ref(&options, media), journal_path) {
        Ok(outcome) => outcome,
        Err(error) => return rendered(failure("", &format!("{error}\n"), 1)),
    };
    match outcome {
        ResolutionOutcome::RouteAppleHealth => rendered(run_apple(media, &options, journal_path)),
        ResolutionOutcome::Skipped {
            reason: solstone_core_import::SkipReason::TimestampRequired,
            detected_timestamp: Some(timestamp),
        } => rendered(failure(
            "",
            &cli_render::timestamp_confirmation(timestamp.as_str()),
            1,
        )),
        ResolutionOutcome::Skipped { reason, .. } => rendered(success(
            cli_render::resolution_skipped(&format!("{reason:?}")),
        )),
        ResolutionOutcome::Resolved {
            source: ResolvedSource::GenericAudio,
            timestamp,
            ..
        } => rendered(run_audio(
            media,
            &options,
            journal_path,
            timestamp.as_str(),
            true,
        )),
        ResolutionOutcome::Resolved {
            source: ResolvedSource::GenericText,
            timestamp,
            stream,
        } => rendered(run_text(media, &options, journal_path, &timestamp, &stream)),
        ResolutionOutcome::Resolved {
            source: ResolvedSource::Registry(source),
            timestamp,
            ..
        } => CliOutcome::Registry(RegistryDispatch {
            media: PathBuf::from(media),
            source,
            timestamp: timestamp.as_str().to_owned(),
            dry_run: options.dry_run,
            force: options.force,
        }),
    }
}

fn rendered(run: CliRun) -> CliOutcome {
    CliOutcome::Rendered(run)
}

/// Current-thread runtime for generic audio import, including the processing wait.
pub fn audio_import_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| error.to_string())
}

fn run_audio(
    media: &str,
    options: &Options,
    journal_path: &Path,
    timestamp: &str,
    wait_for_processing: bool,
) -> CliRun {
    if options.dry_run {
        return failure(
            "",
            "generic audio preview requires the audio import body's preview path\n",
            1,
        );
    }
    let base_timestamp = match NaiveDateTime::parse_from_str(timestamp, "%Y%m%d_%H%M%S") {
        Ok(value) => value,
        Err(_) => return failure("", "timestamp must be YYYYMMDD_HHMMSS format\n", 1),
    };
    let runtime = match audio_import_runtime() {
        Ok(runtime) => runtime,
        Err(error) => return failure("", &format!("audio import runtime failed: {error}\n"), 1),
    };
    // Everything that can fail before the producer starts has now run, so an admitted
    // attempt always has a producer behind it. Admitting above this point would leave a
    // Running row for the full wall-clock bound on an invocation that failed instantly.
    if let Some(message) =
        solstone_core_import::refuse_if_live_running(journal_path, timestamp, "audio")
    {
        return failure("", &format!("{message}\n"), 1);
    }
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // No source hint: `admit_running_attempt` persists one, the web start replays it as
    // `--source <hint>`, and `resolve` refuses any name `RegistrySource` does not know --
    // which has no audio variant. The projection derives "audio" from the publication's
    // `import.audio` stream prefix instead, so the hint buys nothing and would break restart.
    let attempt = match solstone_core_import::admit_running_attempt(
        journal_path,
        timestamp,
        started_at_ms,
        None,
    ) {
        Ok(facts) => facts,
        Err(error) => return failure("", &format!("audio import failed: {error}\n"), 1),
    };
    let request = AudioImportRequest {
        source_media: PathBuf::from(media),
        journal_root: journal_path.to_path_buf(),
        day: timestamp[..8].to_owned(),
        base_timestamp,
        import_id: timestamp.to_owned(),
        stream: "import.audio".to_owned(),
        facet: options.facet.clone(),
        setting: options.setting.clone(),
        // An owner's import waits: returning at once told an owner the import was complete
        // while its segments were still unprocessed, so a failed or stalled segment was never
        // reported. The verb already requires a running solstone, so the consumer these
        // segments wait on exists. Only sync, which files many recordings at once and records
        // each one's outcome in its own state, hands processing to the journal and moves on.
        wait_for_processing,
        stall_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(250),
    };
    let outcome = runtime.block_on(import_audio(request));
    finish_audio_attempt(journal_path, timestamp, attempt.generation, &outcome);
    audio_import_cli_run(outcome)
}

fn run_text(
    media: &str,
    options: &Options,
    journal_path: &Path,
    timestamp: &solstone_core_import::Timestamp,
    stream: &str,
) -> CliRun {
    if options.dry_run {
        return failure(
            "",
            "generic text preview requires a native preview adapter\n",
            1,
        );
    }
    if let Some(error) = refuse_if_live_running(journal_path, timestamp.as_str(), "text") {
        return failure("", &format!("{error}\n"), 1);
    }
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // No source hint: `admit_running_attempt` persists one, the web start replays it as
    // `--source <hint>`, and `resolve` refuses any name `RegistrySource` does not know --
    // which has no text variant. The projection derives "text" from the publication's
    // `import.text` stream prefix instead, so the hint buys nothing and would break restart.
    let generation =
        match admit_running_attempt(journal_path, timestamp.as_str(), started_at_ms, None) {
            Ok(facts) => facts.generation,
            Err(error) => return failure("", &format!("{error}\n"), 1),
        };
    let day_dir = journal_path.join("chronicle").join(timestamp.day());
    if let Err(error) = fs::create_dir_all(&day_dir) {
        let _ = finish_import_attempt(
            journal_path,
            timestamp.as_str(),
            generation,
            "text",
            ImportTerminalInput::Failed(&[]),
        );
        return failure("", &format!("{error}\n"), 1);
    }
    // process_transcript's start_time is a transcript clock (`HH:MM:SS`), not
    // the stamp half (`HHMMSS`). Convert at this seam; do not teach the
    // transcript parser a second format.
    let clock = timestamp.clock();
    let outcome = solstone_core_import::process_transcript(
        Path::new(media),
        &day_dir,
        &clock,
        timestamp.as_str(),
        stream,
        options.facet.as_deref(),
        options.setting.as_deref(),
        None,
    );
    let input = match &outcome {
        TextImportOutcome::Success(work) => ImportTerminalInput::Success(&work.created),
        TextImportOutcome::Failed { created, .. } => ImportTerminalInput::Failed(&created.created),
    };
    let finish = finish_import_attempt(journal_path, timestamp.as_str(), generation, "text", input);
    if let Err(error) = finish {
        return failure("", &format!("{error}\n"), 1);
    }
    match outcome {
        // The terminal record is the authority on whether publication held: a failed
        // publication is recorded as unconfirmed, not reported back as an error.
        TextImportOutcome::Success(_)
            if solstone_core_import::project_import_result(journal_path, timestamp.as_str())
                .status
                != solstone_core_import::ProjectionStatus::Success =>
        {
            failure(
                "",
                "text import saved to your journal, but it could not confirm the entries are ready to search; run it again\n",
                1,
            )
        }
        TextImportOutcome::Success(work) => {
            success(cli_render::generic_text_complete(work.created.len()))
        }
        TextImportOutcome::Failed { error, .. } => failure("", &format!("{error}\n"), 1),
    }
}

fn run_apple(media: &str, options: &Options, journal_path: &Path) -> CliRun {
    let result = if options.dry_run {
        solstone_core_body_ingest::preview_apple(
            Path::new(media),
            options.date_from.as_deref(),
            options.date_to.as_deref(),
        )
    } else {
        solstone_core_body_ingest::save_apple(
            Path::new(media),
            journal_path,
            &solstone_core_body_ingest::AppleImportOptions {
                date_from: options.date_from.clone(),
                date_to: options.date_to.clone(),
                confirm_body_save: options.confirm_body_save,
                force: options.force,
            },
        )
    };
    match result {
        Ok(report) if options.json => success(format!(
            "{}\n",
            json!({"schema":"solstone.body.ingest.result.v1", "source":"apple_health", "mode": if options.dry_run { "preview" } else { "save" }, "bundle_id":report.bundle_id(), "rows":report.rows(), "days":report.days(), "skipped":report.skipped()})
        )),
        Ok(report) => success(format!(
            "Apple Health {} complete.\n  Rows:                {}\n  Days:                {}\n",
            if options.dry_run { "preview" } else { "save" },
            report.rows(),
            report.days().len()
        )),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn options_ref<'a>(options: &'a Options, media: &'a str) -> ResolutionOptions<'a> {
    ResolutionOptions {
        media: Path::new(media),
        source: options.source.as_deref(),
        timestamp: options.timestamp.as_deref(),
        auto: solstone_core_import::AutoTimestamp::from_raw(
            options.auto.as_ref().map(|value| value.as_deref()),
        ),
        dry_run: options.dry_run,
        deterministic_only: options.deterministic_only,
        force: options.force,
    }
}

fn resolve(
    options: ResolutionOptions<'_>,
    journal_path: &Path,
) -> Result<ResolutionOutcome, String> {
    if let Some(source) = options.source
        && solstone_core_import::RegistrySource::from_name(source).is_none()
    {
        return Err(format!("unknown importer source: {source}"));
    }
    if options.source.is_none()
        && options.media.exists()
        && requires_registry_classification(options.media)
        && !solstone_core_body_ingest::detect_apple_source(options.media).map_err(|_| {
            "could not inspect Apple Health export; retry with a valid export".to_owned()
        })?
    {
        return Err(
            "could not tell what kind of export this is; name it with --source, for example --source chatgpt"
                .to_owned(),
        );
    }
    if options.source.is_none()
        && is_generic_media(options.media)
        && !options.dry_run
        && !options.force
        && let Some(manifest) = generic_manifest_summary(journal_path, options.media)?
        && manifest.entry_count > 0
    {
        return Ok(ResolutionOutcome::Skipped {
            reason: solstone_core_import::SkipReason::AlreadyImported,
            detected_timestamp: None,
        });
    }
    let mut seams = ResolutionSeams {
        apple_detector: solstone_core_body_ingest::detect_apple_source,
        // The source crate depends on this crate, so a direct call here would
        // introduce a Cargo cycle. Explicit source selection still reaches the
        // resolver and then returns the named boundary refusal below.
        claims: no_registry_claim,
        deterministic_detector: file_mtime_timestamp,
        model_detector: unavailable_model_timestamp,
        // Generic manifest deduplication is performed above with the resolved
        // journal root. The resolver retains this seam for its library callers.
        manifest_lookup: no_manifest_match,
        generated_timestamp: || {
            solstone_core_import::validate_timestamp(
                &Local::now().format("%Y%m%d_%H%M%S").to_string(),
            )
            .expect("current local timestamp is valid")
        },
    };
    resolve_import(&options, &mut seams).map_err(|error| error.message().into_owned())
}

/// Extensions the reference classifies as audio, plus the video containers its own classifier
/// routes to the audio path (`media_type.startswith(("audio/", "video/"))`).
///
/// The reference sweeps the file-importer registry for these and, when nothing claims them,
/// falls through to the generic audio import. Listing only `m4a` here refused an owner's
/// `.mp3` outright, with a message naming `--source` as the remedy — and no `--source` value
/// reaches generic audio, so the advice could not be followed.
const GENERIC_AUDIO_EXTENSIONS: &[&str] = &[
    "flac", //
    "m4a",  //
    "mov",  //
    "mp3",  //
    "mp4",  //
    "ogg",  //
    "opus", //
    "wav",  //
    "webm", //
];

/// Extensions the reference treats as a generic transcript.
const GENERIC_TEXT_EXTENSIONS: &[&str] = &[
    "md",  //
    "txt", //
];

fn lowercase_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
}

fn requires_registry_classification(path: &Path) -> bool {
    let Some(extension) = lowercase_extension(path) else {
        return true;
    };
    !(GENERIC_AUDIO_EXTENSIONS.contains(&extension.as_str())
        || GENERIC_TEXT_EXTENSIONS.contains(&extension.as_str())
        || extension == "pdf")
}

fn is_generic_media(path: &Path) -> bool {
    lowercase_extension(path).is_some_and(|extension| {
        GENERIC_AUDIO_EXTENSIONS.contains(&extension.as_str())
            || GENERIC_TEXT_EXTENSIONS.contains(&extension.as_str())
    })
}

fn generic_manifest_summary(
    journal_path: &Path,
    media: &Path,
) -> Result<Option<ManifestSummary>, String> {
    let source_hash =
        solstone_core_import::hash_source(media).map_err(|error| error.to_string())?;
    let scan = solstone_core_import::find_manifest_by_hash(journal_path, &source_hash)
        .map_err(|error| error.to_string())?;
    Ok(scan.found.and_then(|found| {
        found
            .manifest
            .get("entry_count")
            .and_then(serde_json::Value::as_u64)
            .map(|entry_count| ManifestSummary { entry_count })
    }))
}

fn no_registry_claim(_: solstone_core_import::RegistrySource, _: &Path) -> Result<bool, ()> {
    Ok(false)
}

fn looks_like_media_path(value: &str) -> bool {
    Path::new(value).exists()
        || value.contains('/')
        || value.contains('\\')
        || Path::new(value).extension().is_some()
}

fn file_mtime_timestamp(
    path: &Path,
    _: Option<&str>,
) -> Option<solstone_core_import::DetectedTimestamp> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let datetime = chrono::DateTime::<Local>::from(modified);
    solstone_core_import::validate_timestamp(&datetime.format("%Y%m%d_%H%M%S").to_string())
        .ok()
        .map(solstone_core_import::DetectedTimestamp::new)
}

fn unavailable_model_timestamp(
    _: &Path,
    _: Option<&str>,
) -> Result<
    Option<solstone_core_import::DetectedTimestamp>,
    solstone_core_import::ModelDetectionError<()>,
> {
    Ok(None)
}

fn no_manifest_match(_: &solstone_core_import::SourceHash) -> Option<ManifestSummary> {
    None
}

fn run_audio_sync(journal_path: &Path, options: &Options) -> CliRun {
    let source_path = options.path.clone().unwrap_or_default();
    let scanner = FilesystemAudioScanner;
    let probe = FilesystemAudioProbe;
    let manifests = FilesystemManifestLookup { journal_path };
    let clock = SystemSyncClock;
    let mut state_writer = FilesystemAudioStateWriter;
    let preview = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut state_writer,
    };
    if !options.save {
        let request = AudioSyncRequest::<SyncPreviewRequest>::new(
            journal_path.to_path_buf(),
            source_path.clone(),
            options.force,
            audio_auto(options),
        );
        let mut seams = preview;
        return match sync_audio_preview(&request, &mut seams) {
            Ok(outcome) => success(cli_render::audio_sync_preview(
                &source_path,
                state_file_count(&outcome.state),
                outcome.errors.len(),
            )),
            Err(error) => failure("", &format!("{error}\n"), 1),
        };
    }
    // A folder sync has no one to confirm each recording's time, so it adopts the time the
    // file carries unless the owner named another rule.
    let auto = match audio_auto(options) {
        AudioAuto::Disabled => AudioAuto::Enabled,
        auto => auto,
    };
    let request = AudioSyncRequest::<SyncSaveRequest>::new(
        journal_path.to_path_buf(),
        source_path,
        options.force,
        auto,
    );
    let mut pipeline = GenericAudioPipeline { journal_path };
    let mut seams = AudioSaveSeams {
        preview,
        pipeline: &mut pipeline,
    };
    match sync_audio_save(&request, &mut seams) {
        Ok(outcome) => sync_saved("Audio", outcome.downloaded, &outcome.errors),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn run_obsidian_sync(journal_path: &Path, options: &Options) -> CliRun {
    let source_path = options.path.clone();
    let candidates = HomeObsidianCandidates::from_environment();
    let scanner = FilesystemObsidianScanner;
    let clock = SystemSyncClock;
    let preview = ObsidianPreviewSeams {
        candidates: &candidates,
        scanner: &scanner,
        clock: &clock,
    };
    if !options.save {
        let request = ObsidianSyncRequest::<SyncPreviewRequest>::new(
            journal_path.to_path_buf(),
            source_path.clone(),
            options.force,
        );
        let mut seams = preview;
        return match sync_obsidian_preview(&request, &mut seams) {
            Ok(outcome) => success(cli_render::obsidian_sync_preview(
                source_path.as_deref(),
                state_file_count(&outcome.state),
                outcome.errors.len(),
            )),
            Err(error) => failure("", &format!("{error}\n"), 1),
        };
    }
    let request = ObsidianSyncRequest::<SyncSaveRequest>::new(
        journal_path.to_path_buf(),
        source_path,
        options.force,
    );
    let mut writer = JournalObsidianWriter { journal_path };
    let mut seams = ObsidianSaveSeams {
        preview,
        writer: &mut writer,
    };
    match sync_obsidian_save(&request, &mut seams) {
        Ok(outcome) => sync_saved("Obsidian", outcome.imported, &outcome.errors),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn run_plaud_sync(journal_path: &Path, options: &Options) -> CliRun {
    let credential = ConfiguredPlaudCredential::read(journal_path);
    let mut catalogue = PlaudApi::new();
    let manifests = ImportFilenameMatches { journal_path };
    let clock = SystemSyncClock;
    let mut state_writer = FilesystemPlaudStateWriter;
    let preview = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut state_writer,
    };
    if !options.save {
        let request = PlaudSyncRequest::<SyncPreviewRequest>::new(journal_path.to_path_buf());
        let mut seams = preview;
        return match sync_plaud_preview(&request, &mut seams) {
            Ok(outcome) => success(cli_render::plaud_sync_preview(state_file_count(
                &outcome.state,
            ))),
            Err(error) => failure("", &format!("{error}\n"), 1),
        };
    }
    let request = PlaudSyncRequest::<SyncSaveRequest>::new(journal_path.to_path_buf());
    let mut download = PlaudApi::new();
    let mut pipeline = GenericAudioPipeline { journal_path };
    let mut seams = PlaudSaveSeams {
        preview,
        download: &mut download,
        pipeline: &mut pipeline,
    };
    match sync_plaud_save(&request, &mut seams) {
        Ok(outcome) => sync_saved("Plaud", outcome.downloaded, &outcome.errors),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

/// A sync that saved: a count on stdout, and each item that failed on stderr, failing the run.
fn sync_saved(backend: &str, saved: u64, errors: &[String]) -> CliRun {
    let stdout = cli_render::sync_save_complete(backend, saved, errors.len());
    if errors.is_empty() {
        return success(stdout);
    }
    let stderr = errors
        .iter()
        .map(|error| format!("{error}\n"))
        .collect::<String>();
    failure(&stdout, &stderr, 1)
}

/// Sync hands each recording to the generic audio import, which files it as its own import.
struct GenericAudioPipeline<'a> {
    journal_path: &'a Path,
}

impl ImportPipeline for GenericAudioPipeline<'_> {
    fn import_one(
        &mut self,
        request: PipelineImportRequest<'_>,
    ) -> Result<PipelineOutcome, String> {
        let media = request.source.to_string_lossy().into_owned();
        let options = Options {
            media: Some(media.clone()),
            timestamp: request.timestamp.map(str::to_owned),
            auto: match request.auto {
                PipelineAuto::Enabled => Some(None),
                PipelineAuto::Disabled => None,
                PipelineAuto::Value(value) => Some(Some(value.to_owned())),
            },
            ..Options::default()
        };
        match resolve(options_ref(&options, &media), self.journal_path)? {
            ResolutionOutcome::Resolved {
                source: ResolvedSource::GenericAudio,
                timestamp,
                ..
            } => {
                let run = run_audio(
                    &media,
                    &options,
                    self.journal_path,
                    timestamp.as_str(),
                    false,
                );
                if run.exit_code == 0 {
                    Ok(PipelineOutcome::Imported)
                } else {
                    Err(run.stderr.trim().to_owned())
                }
            }
            ResolutionOutcome::Skipped { reason, .. } => Ok(PipelineOutcome::Skipped {
                reason: format!("{reason:?}"),
            }),
            ResolutionOutcome::Resolved { .. } | ResolutionOutcome::RouteAppleHealth => {
                Ok(PipelineOutcome::Unrecognized)
            }
        }
    }
}

fn audio_auto(options: &Options) -> AudioAuto {
    match options.auto.as_ref() {
        Some(None) => AudioAuto::Enabled,
        Some(Some(value)) => AudioAuto::Value(value.clone()),
        None => AudioAuto::Disabled,
    }
}

fn state_file_count(state: &solstone_core_import::SyncState) -> usize {
    state
        .root()
        .get("files")
        .and_then(serde_json::Value::as_object)
        .map_or(0, serde_json::Map::len)
}

struct SystemSyncClock;

impl SyncClock for SystemSyncClock {
    fn now(&self) -> String {
        Local::now().to_rfc3339()
    }
}

struct FilesystemAudioScanner;

impl DirectoryScanner for FilesystemAudioScanner {
    fn audio_candidates(&self, root: &Path) -> Result<Vec<AudioCandidate>, String> {
        let mut candidates = Vec::new();
        collect_audio_candidates(root, root, &mut candidates)?;
        Ok(candidates)
    }
}

fn collect_audio_candidates(
    root: &Path,
    directory: &Path,
    candidates: &mut Vec<AudioCandidate>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_audio_candidates(root, &path, candidates)?;
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        if !path.is_file() || !matches!(extension.as_deref(), Some("m4a" | "mp3" | "wav" | "opus"))
        {
            continue;
        }
        let relative_path = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("audio filename is not UTF-8: {}", path.display()))?
            .to_owned();
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        let source_hash = solstone_core_import::hash_source(&path)
            .map_err(|error| error.to_string())?
            .into_inner();
        candidates.push(AudioCandidate {
            relative_path,
            source: path,
            filename,
            filesize: metadata.len(),
            source_hash,
        });
    }
    Ok(())
}

struct FilesystemAudioProbe;

impl AudioProbe for FilesystemAudioProbe {
    fn duration_seconds(&self, source: &Path) -> Result<Option<f64>, String> {
        ffmpeg::init().map_err(|error| error.to_string())?;
        let input = crate::audio::open_input(source).map_err(|error| error.to_string())?;
        let duration = input.duration();
        if duration == ffmpeg::ffi::AV_NOPTS_VALUE {
            return Ok(None);
        }
        Ok(Some(duration as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)))
    }
}

struct FilesystemManifestLookup<'a> {
    journal_path: &'a Path,
}

impl ManifestLookup for FilesystemManifestLookup<'_> {
    fn imported_hash(&self, source_hash: &str) -> bool {
        solstone_core_import::find_manifest_by_hash(
            self.journal_path,
            &solstone_core_import::SourceHash::new(source_hash.to_owned()),
        )
        .ok()
        .and_then(|scan| scan.found)
        .is_some()
    }
}

/// The vault folders a sync looks in when it has no path and no vault of its own yet.
struct HomeObsidianCandidates {
    paths: Vec<PathBuf>,
}

impl HomeObsidianCandidates {
    fn from_environment() -> Self {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        Self {
            paths: home
                .map(|home| {
                    vec![
                        home.join("Documents").join("Obsidian"),
                        home.join("Obsidian"),
                    ]
                })
                .unwrap_or_default(),
        }
    }
}

impl ObsidianHomeCandidates for HomeObsidianCandidates {
    fn candidates(&self) -> &[PathBuf] {
        &self.paths
    }
}

/// Sync reads a vault the way a vault import does, so both skip the same folders.
struct FilesystemObsidianScanner;

impl ObsidianScanner for FilesystemObsidianScanner {
    fn is_directory(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn notes(&self, vault: &Path) -> Result<Vec<ObsidianNote>, String> {
        obsidian::collect_notes(vault)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|note| {
                let path = vault.join(&note.source_path);
                Ok(ObsidianNote {
                    relative_path: note.source_path.to_string_lossy().into_owned(),
                    filename: path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .ok_or_else(|| format!("note filename is not UTF-8: {}", path.display()))?
                        .to_owned(),
                    title: note.title,
                    modified_at: note.modified.timestamp_micros() as f64 / 1_000_000.0,
                    content_hash: solstone_core_import::hash_source(&path)
                        .map_err(|error| error.to_string())?
                        .into_inner(),
                })
            })
            .collect()
    }
}

/// Sync writes each changed note as it stands now, at the moment the file last changed.
struct JournalObsidianWriter<'a> {
    journal_path: &'a Path,
}

impl ObsidianWriter for JournalObsidianWriter<'_> {
    fn import_note(&mut self, vault: &Path, note: &ObsidianNote) -> Result<u64, String> {
        let entry = obsidian::read_note(vault, &vault.join(&note.relative_path))
            .map_err(|error| error.to_string())?;
        let rendered = obsidian::render_notes(vec![entry], &Local);
        let written = save::write_rendered(self.journal_path, None, &rendered);
        if let Some(error) = written.error {
            return Err(error.to_string());
        }
        let segments = written
            .created
            .iter()
            .map(|file| file.created_segment())
            .collect::<Vec<_>>();
        if segments.is_empty() {
            return Ok(0);
        }
        let files = written
            .created
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let record = publish(PublicationInput {
            journal: self.journal_path,
            import_dir: None,
            import_id: "sync:obsidian",
            importer: "obsidian",
            revision: None,
            segments: &segments,
            files_created: &files,
            may_write_record: None,
        })
        .map_err(|error| error.to_string())?;
        if record.status != PublicationStatus::Success {
            return Err(
                "saved to your journal, but it could not confirm the note is ready to search; run it again".to_owned(),
            );
        }
        Ok(u64::try_from(segments.len()).expect("segment count fits u64"))
    }
}

/// The owner's Plaud token, as Settings stores it, or from the environment.
struct ConfiguredPlaudCredential {
    token: Option<String>,
}

impl ConfiguredPlaudCredential {
    const KEY: &'static str = "PLAUD_ACCESS_TOKEN";

    fn read(journal_path: &Path) -> Self {
        let configured = solstone_core_journal_config::read_journal_config(journal_path)
            .ok()
            .and_then(|read| read.config)
            .and_then(|config| {
                config
                    .get("env")
                    .and_then(|env| env.get(Self::KEY))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let token = configured
            .or_else(|| std::env::var(Self::KEY).ok())
            .filter(|token| !token.trim().is_empty());
        Self { token }
    }
}

impl PlaudCredential for ConfiguredPlaudCredential {
    fn access_token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

const PLAUD_API: &str = "https://api.plaud.ai";
/// A recording download is bounded, so a stalled transfer cannot hold a sync forever.
const PLAUD_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The Plaud web API: the owner's recording list, and each recording's download.
struct PlaudApi {
    agent: ureq::Agent,
}

impl PlaudApi {
    fn new() -> Self {
        let timeout = Some(Duration::from_secs(30));
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(timeout)
            .timeout_recv_response(timeout)
            .timeout_recv_body(Some(PLAUD_DOWNLOAD_TIMEOUT))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn get_json(
        &self,
        url: &str,
        token: &str,
        failure: PlaudFailureKind,
    ) -> Result<Value, PlaudFailureKind> {
        let mut response = self
            .agent
            .get(url)
            .header("accept", "application/json, text/plain, */*")
            .header("authorization", format!("bearer {token}"))
            .header("app-platform", "web")
            .call()
            .map_err(|_| failure)?;
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(PlaudFailureKind::TokenRefused),
            _ => return Err(failure),
        }
        let value: Value =
            serde_json::from_reader(response.body_mut().as_reader()).map_err(|_| failure)?;
        if value.get("status").and_then(Value::as_i64) == Some(0) {
            Ok(value)
        } else {
            Err(failure)
        }
    }
}

impl PlaudCatalogue for PlaudApi {
    fn list_files(&mut self, token: &str) -> Result<Vec<PlaudFile>, PlaudFailureKind> {
        let url = format!(
            "{PLAUD_API}/file/simple/web?skip=0&limit=99999&is_trash=2&sort_by=start_time&is_desc=true"
        );
        let value = self.get_json(&url, token, PlaudFailureKind::Catalogue)?;
        let files = value
            .get("data_file_list")
            .and_then(Value::as_array)
            .ok_or(PlaudFailureKind::Catalogue)?;
        Ok(files.iter().filter_map(plaud_file).collect())
    }
}

impl PlaudDownload for PlaudApi {
    fn temporary_url(&mut self, token: &str, file_id: &str) -> Result<String, PlaudFailureKind> {
        let value = self.get_json(
            &format!("{PLAUD_API}/file/temp-url/{file_id}"),
            token,
            PlaudFailureKind::TemporaryUrl,
        )?;
        value
            .get("temp_url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .ok_or(PlaudFailureKind::TemporaryUrl)
    }

    fn download(&mut self, url: &str, destination: &Path) -> Result<(), PlaudFailureKind> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|_| PlaudFailureKind::Download)?;
        if response.status() != 200 {
            return Err(PlaudFailureKind::Download);
        }
        // Stream beside the destination, then rename: a failed transfer leaves no recording
        // that looks whole.
        let partial = destination.with_extension("part");
        let written = fs::File::create(&partial)
            .and_then(|mut file| {
                std::io::copy(&mut response.into_body().into_reader(), &mut file)?;
                file.sync_all()
            })
            .and_then(|()| fs::rename(&partial, destination));
        if written.is_err() {
            let _ = fs::remove_file(&partial);
            return Err(PlaudFailureKind::Download);
        }
        Ok(())
    }
}

fn plaud_file(value: &Value) -> Option<PlaudFile> {
    let number = |key: &str| value.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Some(PlaudFile {
        id: value.get("id")?.as_str()?.to_owned(),
        filename: text("filename"),
        fullname: text("fullname"),
        filesize: value.get("filesize").and_then(Value::as_u64).unwrap_or(0),
        start_time: number("start_time"),
        duration: number("duration"),
        is_trash: value
            .get("is_trash")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// A Plaud recording the owner already imported by hand is matched by its file name, so a
/// sync does not bring it in a second time.
struct ImportFilenameMatches<'a> {
    journal_path: &'a Path,
}

impl PlaudManifestLookup for ImportFilenameMatches<'_> {
    fn matching_imports(
        &self,
        files: &[PlaudFile],
    ) -> Result<std::collections::BTreeMap<String, String>, PlaudFailureKind> {
        let mut imported = std::collections::HashMap::new();
        let entries = match fs::read_dir(self.journal_path.join("imports")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(std::collections::BTreeMap::new());
            }
            Err(_) => return Err(PlaudFailureKind::Manifest),
        };
        for entry in entries.filter_map(Result::ok) {
            let Some(import_id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(original) = fs::read(entry.path().join("import.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|metadata| {
                    metadata
                        .get("original_filename")?
                        .as_str()
                        .map(str::to_owned)
                })
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let stem = Path::new(&original)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            for name in [original, sanitize_filename(&stem), stem] {
                if !name.is_empty() {
                    imported.insert(name, import_id.clone());
                }
            }
        }
        Ok(files
            .iter()
            .filter_map(|file| {
                let extension = Path::new(&file.fullname)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map_or_else(|| ".opus".to_owned(), |extension| format!(".{extension}"));
                let hash_stem = Path::new(&file.fullname)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_owned();
                let sanitized = sanitize_filename(&file.filename);
                [
                    file.filename.clone(),
                    hash_stem,
                    sanitized.clone(),
                    format!("{}{extension}", file.filename),
                    format!("{sanitized}{extension}"),
                ]
                .into_iter()
                .find_map(|candidate| imported.get(&candidate).cloned())
                .map(|import_id| (file.id.clone(), import_id))
            })
            .collect())
    }
}

#[derive(Default)]
struct Options {
    media: Option<String>,
    timestamp: Option<String>,
    source: Option<String>,
    sync: Option<String>,
    connect: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    path: Option<PathBuf>,
    facet: Option<String>,
    setting: Option<String>,
    auto: Option<Option<String>>,
    window_days: Option<u64>,
    force: bool,
    dry_run: bool,
    confirm_body_save: bool,
    save: bool,
    scheduled: bool,
    list_importers: bool,
    backends: bool,
    json: bool,
    deterministic_only: bool,
}

enum ParsedCommand {
    Help,
    ListImporters { json: bool },
    Backends,
    Connect { backend: String },
    Sync { backend: String, options: Options },
    Import(Options),
}

fn parse_arguments(args: &[String]) -> Result<ParsedCommand, String> {
    let mut options = Options::default();
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "-h" || argument == "--help" {
            return Ok(ParsedCommand::Help);
        }
        if argument == "--" {
            positionals.extend(args[index + 1..].iter().cloned());
            break;
        }
        if let Some((name, value)) = argument.split_once('=') {
            assign_value(&mut options, name, value)?;
        } else if takes_value(argument) {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("argument {argument}: expected one argument"))?;
            assign_value(&mut options, argument, value)?;
            index += 1;
        } else if argument == "--auto" {
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with('-') && !looks_like_media_path(value))
                .cloned();
            if value.is_some() {
                index += 1;
            }
            options.auto = Some(value);
        } else if assign_flag(&mut options, argument) {
        } else if argument.starts_with('-') {
            return Err(format!("unrecognized arguments: {argument}"));
        } else {
            positionals.push(argument.clone());
        }
        index += 1;
    }
    match positionals.as_slice() {
        [] => {}
        [media] => options.media = Some(media.clone()),
        [media, timestamp] => {
            options.media = Some(media.clone());
            options.timestamp = Some(timestamp.clone());
        }
        [media, timestamp, extras @ ..] => {
            options.media = Some(media.clone());
            options.timestamp = Some(timestamp.clone());
            return Err(format!("unrecognized arguments: {}", extras.join(" ")));
        }
    }
    if options.list_importers {
        return Ok(ParsedCommand::ListImporters { json: options.json });
    }
    if options.backends {
        return Ok(ParsedCommand::Backends);
    }
    if let Some(backend) = options.connect.clone() {
        return Ok(ParsedCommand::Connect { backend });
    }
    if let Some(backend) = options.sync.clone() {
        return Ok(ParsedCommand::Sync { backend, options });
    }
    Ok(ParsedCommand::Import(options))
}

fn takes_value(argument: &str) -> bool {
    matches!(
        argument,
        "--timestamp"
            | "--facet"
            | "--setting"
            | "--source"
            | "--sync"
            | "--path"
            | "--window-days"
            | "--connect"
            | "--date-from"
            | "--date-to"
    )
}

fn assign_value(options: &mut Options, name: &str, value: &str) -> Result<(), String> {
    match name {
        "--timestamp" => options.timestamp = Some(value.to_owned()),
        "--source" => options.source = Some(value.to_owned()),
        "--sync" => options.sync = Some(value.to_owned()),
        "--connect" => options.connect = Some(value.to_owned()),
        "--date-from" => options.date_from = Some(value.to_owned()),
        "--date-to" => options.date_to = Some(value.to_owned()),
        "--path" => options.path = Some(PathBuf::from(value)),
        "--window-days" => {
            options.window_days = Some(
                value
                    .parse()
                    .map_err(|_| format!("argument --window-days: invalid int value: '{value}'"))?,
            )
        }
        "--facet" => options.facet = Some(value.to_owned()),
        "--setting" => options.setting = Some(value.to_owned()),
        _ => return Err(format!("unrecognized arguments: {name}={value}")),
    }
    Ok(())
}

fn assign_flag(options: &mut Options, argument: &str) -> bool {
    match argument {
        "--force" => options.force = true,
        "--dry-run" => options.dry_run = true,
        "--confirm-body-save" | "--confirm-health-save" => options.confirm_body_save = true,
        "--with-day-summaries" | "-v" | "--verbose" | "-d" | "--debug" => {}
        "--deterministic-only" => options.deterministic_only = true,
        "--backends" => options.backends = true,
        "--save" => options.save = true,
        "--scheduled" => options.scheduled = true,
        "--list-importers" => options.list_importers = true,
        "--json" => options.json = true,
        _ => return false,
    }
    true
}

/// Map one audio-import outcome onto the importer CLI contract.
pub fn audio_import_cli_run(
    result: Result<crate::audio::AudioImportOutcome, solstone_core_import::ImportError>,
) -> CliRun {
    match result {
        Ok(outcome) => {
            let processing = &outcome.created().processing;
            if !processing.failed_segments.is_empty() || !processing.stalled_segments.is_empty() {
                let mut keys = processing.failed_segments.clone();
                keys.extend(processing.stalled_segments.iter().cloned());
                return failure(
                    "",
                    &format!("audio import processing failed: {}\n", keys.join(", ")),
                    1,
                );
            }
            success(format!("Generic audio import complete: {outcome:?}\n"))
        }
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn argparse_error(arguments: String) -> CliRun {
    failure(
        "",
        &format!(
            "usage: journal importer [-h] [options] media [timestamp]\njournal importer: error: {arguments}\n"
        ),
        2,
    )
}
fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}
fn failure(stdout: &str, stderr: &str, exit_code: i32) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(id: &str, filename: &str, fullname: &str) -> PlaudFile {
        plaud_file(&serde_json::json!({
            "id": id,
            "filename": filename,
            "fullname": fullname,
            "filesize": 1024,
            "start_time": 1_773_230_400_000_u64,
            "duration": 95_000,
            "is_trash": false,
        }))
        .expect("a catalogue row with an id is a recording")
    }

    #[test]
    fn a_catalogue_row_becomes_a_recording_and_a_row_without_an_id_does_not() {
        let file = recording("abc", "Team standup", "abc.opus");
        assert_eq!(file.id, "abc");
        assert_eq!(file.filename, "Team standup");
        assert_eq!(file.start_time, 1_773_230_400_000.0);
        assert_eq!(file.duration, 95_000.0);
        assert!(!file.is_trash);
        assert!(plaud_file(&serde_json::json!({"filename": "no id"})).is_none());
    }

    #[test]
    fn a_recording_already_imported_by_hand_matches_its_import() {
        let journal = tempfile::TempDir::new().unwrap();
        for (import_id, original) in [
            ("20260311_120000", "Team standup.opus"),
            ("20260312_090000", "Dentist_call.mp3"),
        ] {
            let directory = journal.path().join("imports").join(import_id);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("import.json"),
                serde_json::json!({ "original_filename": original }).to_string(),
            )
            .unwrap();
        }
        let files = [
            recording("by-name", "Team standup", "by-name.opus"),
            recording("by-sanitized-name", "Dentist call", "by-sanitized-name.mp3"),
            recording("new", "Board meeting", "new.opus"),
        ];

        let matches = ImportFilenameMatches {
            journal_path: journal.path(),
        }
        .matching_imports(&files)
        .unwrap();

        assert_eq!(
            matches.get("by-name").map(String::as_str),
            Some("20260311_120000")
        );
        assert_eq!(
            matches.get("by-sanitized-name").map(String::as_str),
            Some("20260312_090000")
        );
        assert!(!matches.contains_key("new"));
    }
}
