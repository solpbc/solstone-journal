// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal importer argv parsing and dispatch.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use ffmpeg_next as ffmpeg;
use serde_json::json;
use solstone_core_segment::{
    SUPERVISOR_MESSAGE, SupervisorRefusal, require_solstone, require_solstone_with,
};

use solstone_core_import::cli_journal_source;
use solstone_core_import::cli_render::{self, CliRun};
use solstone_core_import::connect::{OuraConnectRequest, connect_oura};
use solstone_core_import::contract::{AudioAuto, SyncPreviewRequest};
use solstone_core_import::detect::{
    ManifestSummary, RegistrySource, ResolutionOptions, ResolutionOutcome, ResolutionSeams,
    ResolvedSource, resolve_import,
};
use solstone_core_import::sync_audio::{
    AudioCandidate, AudioPreviewSeams, AudioProbe, AudioSyncRequest, DirectoryScanner,
    FilesystemAudioStateWriter, ManifestLookup, sync_audio_preview,
};
use solstone_core_import::sync_obsidian::{
    ObsidianHomeCandidates, ObsidianNote, ObsidianPreviewSeams, ObsidianScanner,
    ObsidianSyncRequest, sync_obsidian_preview,
};
use solstone_core_import::sync_plaud::{
    FilesystemPlaudStateWriter, PlaudCatalogue, PlaudCredential, PlaudFailureKind,
    PlaudManifestLookup, PlaudPreviewSeams, PlaudSyncRequest, SyncClock, sync_plaud_preview,
};

use crate::audio::{AudioImportRequest, import_audio};

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
    if media == "journal-source" {
        return rendered(cli_journal_source::run_cli(&options.extra, journal_path));
    }
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
        } => rendered(run_audio(media, &options, journal_path, timestamp.as_str())),
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

fn run_audio(media: &str, options: &Options, journal_path: &Path, timestamp: &str) -> CliRun {
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
    let request = AudioImportRequest {
        source_media: PathBuf::from(media),
        journal_root: journal_path.to_path_buf(),
        day: timestamp[..8].to_owned(),
        base_timestamp,
        import_id: timestamp.to_owned(),
        stream: "import.audio".to_owned(),
        facet: options.facet.clone(),
        setting: options.setting.clone(),
        // The reference waits by default; only the audio-folder sync backend turns this off.
        // Returning immediately told an owner the import was complete while its segments were
        // still unprocessed, so nothing had reached a stream or the index yet and a failed or
        // stalled segment was never reported at all. The verb already requires a running
        // solstone, so the consumer these segments wait on is guaranteed to exist.
        wait_for_processing: true,
        stall_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(250),
    };
    audio_import_cli_run(runtime.block_on(import_audio(request)))
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
    let day_dir = journal_path.join("chronicle").join(timestamp.day());
    if let Err(error) = fs::create_dir_all(&day_dir) {
        return failure("", &format!("{error}\n"), 1);
    }
    // process_transcript's start_time is a transcript clock (`HH:MM:SS`), not
    // the stamp half (`HHMMSS`). Convert at this seam; do not teach the
    // transcript parser a second format.
    let clock = timestamp.clock();
    match solstone_core_import::process_transcript(
        Path::new(media),
        &day_dir,
        &clock,
        timestamp.as_str(),
        stream,
        options.facet.as_deref(),
        options.setting.as_deref(),
        None,
    ) {
        Ok(created) => success(cli_render::generic_text_complete(created.len())),
        Err(error) => failure("", &format!("{error}\n"), 1),
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
            "automatic source classification requires solstone-core-import-sources registry claims; specify --source"
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
    if options.save {
        return failure(
            "",
            "audio sync save requires a native import pipeline adapter\n",
            1,
        );
    }
    let source_path = options.path.clone().unwrap_or_default();
    let request = AudioSyncRequest::<SyncPreviewRequest>::new(
        journal_path.to_path_buf(),
        source_path.clone(),
        options.force,
        audio_auto(options),
    );
    let scanner = FilesystemAudioScanner;
    let probe = FilesystemAudioProbe;
    let manifests = FilesystemManifestLookup { journal_path };
    let clock = SystemSyncClock;
    let mut state_writer = FilesystemAudioStateWriter;
    let mut seams = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut state_writer,
    };
    match sync_audio_preview(&request, &mut seams) {
        Ok(outcome) => success(cli_render::audio_sync_preview(
            &source_path,
            state_file_count(&outcome.state),
            outcome.errors.len(),
        )),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn run_obsidian_sync(journal_path: &Path, options: &Options) -> CliRun {
    if options.save {
        return failure(
            "",
            "Obsidian sync save requires a native note import adapter\n",
            1,
        );
    }
    let source_path = options.path.clone();
    let request = ObsidianSyncRequest::<SyncPreviewRequest>::new(
        journal_path.to_path_buf(),
        source_path.clone(),
        options.force,
    );
    let candidates = EmptyObsidianCandidates;
    let scanner = FilesystemObsidianScanner;
    let clock = SystemSyncClock;
    let mut seams = ObsidianPreviewSeams {
        candidates: &candidates,
        scanner: &scanner,
        clock: &clock,
    };
    match sync_obsidian_preview(&request, &mut seams) {
        Ok(outcome) => success(cli_render::obsidian_sync_preview(
            source_path.as_deref(),
            state_file_count(&outcome.state),
            outcome.errors.len(),
        )),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn run_plaud_sync(journal_path: &Path, options: &Options) -> CliRun {
    if options.save {
        return failure(
            "",
            "Plaud sync save requires native credential, download, and import pipeline adapters\n",
            1,
        );
    }
    let credential = MissingPlaudCredential;
    let mut catalogue = UnusedPlaudCatalogue;
    let manifests = EmptyPlaudManifestLookup;
    let clock = SystemSyncClock;
    let mut state_writer = FilesystemPlaudStateWriter;
    let mut seams = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut state_writer,
    };
    let request = PlaudSyncRequest::<SyncPreviewRequest>::new(journal_path.to_path_buf());
    match sync_plaud_preview(&request, &mut seams) {
        Ok(outcome) => success(cli_render::plaud_sync_preview(state_file_count(
            &outcome.state,
        ))),
        Err(error) => failure("", &format!("{error}\n"), 1),
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
        let input = ffmpeg::format::input(source).map_err(|error| error.to_string())?;
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

struct EmptyObsidianCandidates;

impl ObsidianHomeCandidates for EmptyObsidianCandidates {
    fn candidates(&self) -> &[PathBuf] {
        &[]
    }
}

struct FilesystemObsidianScanner;

impl ObsidianScanner for FilesystemObsidianScanner {
    fn is_directory(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn notes(&self, vault: &Path) -> Result<Vec<ObsidianNote>, String> {
        let mut notes = Vec::new();
        collect_obsidian_notes(vault, vault, &mut notes)?;
        Ok(notes)
    }
}

fn collect_obsidian_notes(
    root: &Path,
    directory: &Path,
    notes: &mut Vec<ObsidianNote>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_obsidian_notes(root, &path, notes)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let content = fs::read(&path).map_err(|error| error.to_string())?;
        let title = String::from_utf8_lossy(&content)
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("note")
            })
            .to_owned();
        let relative_path = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("note filename is not UTF-8: {}", path.display()))?
            .to_owned();
        let modified_at = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .map_err(|error| error.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs_f64();
        let content_hash = solstone_core_import::hash_source(&path)
            .map_err(|error| error.to_string())?
            .into_inner();
        notes.push(ObsidianNote {
            relative_path,
            filename,
            title,
            modified_at,
            content_hash,
        });
    }
    Ok(())
}

struct MissingPlaudCredential;

impl PlaudCredential for MissingPlaudCredential {
    fn access_token(&self) -> Option<&str> {
        None
    }
}

struct UnusedPlaudCatalogue;

impl PlaudCatalogue for UnusedPlaudCatalogue {
    fn list_files(
        &mut self,
        _token: &str,
    ) -> Result<Vec<solstone_core_import::sync_plaud::PlaudFile>, PlaudFailureKind> {
        unreachable!("Plaud catalogue is not called without a credential")
    }
}

struct EmptyPlaudManifestLookup;

impl PlaudManifestLookup for EmptyPlaudManifestLookup {
    fn matching_imports(
        &self,
        _files: &[solstone_core_import::sync_plaud::PlaudFile],
    ) -> Result<std::collections::BTreeMap<String, String>, PlaudFailureKind> {
        Ok(std::collections::BTreeMap::new())
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
    extra: Vec<String>,
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
    if positionals
        .first()
        .is_some_and(|value| value == "journal-source")
    {
        options.media = Some("journal-source".to_owned());
        options.extra = positionals.into_iter().skip(1).collect();
        return Ok(ParsedCommand::Import(options));
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
