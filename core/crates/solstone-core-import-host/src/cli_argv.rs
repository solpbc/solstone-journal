// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal importer argv parsing and dispatch.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use solstone_core_segment::{
    SUPERVISOR_MESSAGE, SupervisorRefusal, require_solstone, require_solstone_with,
};

use solstone_core_import::cli_render::{self, CliRun};
use solstone_core_import::connect::{OuraConnectRequest, connect_oura, needs_oura_app};
use solstone_core_import::contract::{SyncPreviewRequest, SyncSaveRequest};
use solstone_core_import::detect::{
    ManifestSummary, RegistrySource, ResolutionOptions, ResolutionOutcome, ResolutionSeams,
    ResolvedSource, resolve_import,
};
use solstone_core_import::sync_plaud::{
    FilesystemPlaudStateWriter, ImportPipeline, PipelineAuto, PipelineImportRequest,
    PipelineOutcome, PlaudCatalogue, PlaudCredential, PlaudDownload, PlaudFailureKind, PlaudFile,
    PlaudManifestLookup, PlaudPreviewSeams, PlaudSaveSeams, PlaudSyncError, PlaudSyncRequest,
    SyncClock, sanitize_filename, sync_plaud_preview, sync_plaud_save,
};

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
    /// The import crate ran one import to its end under `import_id`.
    Imported { run: CliRun, import_id: String },
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
    pub json: bool,
    pub force: bool,
    pub owner_timestamp: Option<String>,
    pub fallback_timestamp: Option<String>,
    /// The facet a Strava import files its workouts under.
    pub facet: ImportFacet,
    /// How many already-listed workouts the owner was shown would move, when
    /// they were shown a count before the run. It moves no more than that.
    pub listed_moves_shown: Option<usize>,
}

/// The facet one Strava import files its workouts under.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub enum ImportFacet {
    /// Nobody chose for this run: the remembered default while it is still
    /// enabled, otherwise none. Never creates a facet and never asks.
    #[default]
    Unattended,
    /// The owner chose no facet for this run.
    None,
    /// The owner chose this facet for this run.
    Facet(String),
}

impl ImportFacet {
    /// What the run's preview binds: nothing when nobody chose, otherwise the
    /// choice in the stored form of a remembered default.
    #[must_use]
    pub fn preview_value(&self) -> Option<serde_json::Value> {
        match self {
            Self::Unattended => None,
            Self::None => Some(solstone_core_journal_config::body_facet_value(
                &solstone_core_journal_config::BodyFacetChoice::None,
            )),
            Self::Facet(facet) => Some(solstone_core_journal_config::body_facet_value(
                &solstone_core_journal_config::BodyFacetChoice::Facet(facet.clone()),
            )),
        }
    }
}

/// A manual CLI import prints its size before the mutating invocation. Recurring
/// connector commands retain their existing authorization and execution contracts.
///
/// The supervisor preflight answers before the source is read, exactly as it does
/// for the import itself, so a refused import keeps its contracted output (a
/// silent 75 under a spawning supervisor) rather than a preview error. `Err` is the
/// complete run to report.
pub fn preview_arguments(args: &[String], journal: &Path) -> Result<Option<Vec<String>>, CliRun> {
    preview_arguments_with(args, journal, || require_solstone(journal))
}

/// Shape preview arguments with the caller's supervisor preflight.
///
/// The embedded entry passes a preflight that succeeds without probing. The
/// desktop adapter passes [`require_solstone`].
pub fn preview_arguments_with(
    args: &[String],
    journal: &Path,
    preflight: impl FnOnce() -> Result<(), SupervisorRefusal>,
) -> Result<Option<Vec<String>>, CliRun> {
    match parse_arguments(args) {
        Ok(ParsedCommand::Import(options)) if !options.dry_run => {
            let Some(media) = options.media else {
                return Ok(None);
            };
            if let Err(refusal) = preflight() {
                return Err(refusal_run(refusal));
            }
            let digest = match options.snapshot_digest {
                Some(digest) => digest,
                None => solstone_core_import::snapshot::fingerprint(Path::new(&media)).map_err(
                    |error| failure("", &format!("could not preview import: {error}\n"), 1),
                )?,
            };
            let mut preview = vec![
                "--dry-run".to_owned(),
                "--snapshot-digest".to_owned(),
                digest,
                "--preview-zone".to_owned(),
                options.preview_zone.unwrap_or_else(|| {
                    solstone_core_journal_config::owner_zone(journal)
                        .iana_name()
                        .unwrap_or_default()
                        .to_owned()
                }),
            ];
            preview.extend_from_slice(args);
            Ok(Some(preview))
        }
        _ => Ok(None),
    }
}

/// Resolve a force-import day notice through the same argument grammar and supervisor
/// preflight as execution. This reads no source until the supervisor allows the import.
pub fn force_day_notice_for_args(
    args: &[String],
    journal: &Path,
) -> Result<Option<String>, CliRun> {
    force_day_notice_with(args, journal, || require_solstone(journal))
}

/// Resolve a force-import day notice with the caller's supervisor preflight.
///
/// The embedded entry does not probe. The desktop adapter passes
/// [`require_solstone`].
pub fn force_day_notice_with(
    args: &[String],
    journal: &Path,
    preflight: impl FnOnce() -> Result<(), SupervisorRefusal>,
) -> Result<Option<String>, CliRun> {
    let Ok(ParsedCommand::Import(mut options)) = parse_arguments(args) else {
        return Ok(None);
    };
    if !options.force || options.dry_run {
        return Ok(None);
    }
    if let Err(refusal) = preflight() {
        return Err(refusal_run(refusal));
    }
    if owner_instant(&mut options, journal).is_err() {
        return Ok(None);
    }
    let Some(media) = options
        .media
        .as_deref()
        .map(Path::new)
        .filter(|path| path.is_file())
    else {
        return Ok(None);
    };
    let extension = media
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let selected = options
        .source
        .as_deref()
        .and_then(RegistrySource::from_name)
        .or_else(|| {
            if ["txt", "md", "m4a"].contains(&extension.as_str()) {
                return None;
            }
            solstone_core_import::detect::ORDERED_FILE_IMPORTER_NAMES
                .iter()
                .filter_map(|name| RegistrySource::from_name(name))
                .find(|source| {
                    solstone_core_import_sources::registry::claims(*source, media).unwrap_or(false)
                })
        });
    if selected
        .is_some_and(|source| !matches!(source, RegistrySource::Image | RegistrySource::Document))
    {
        // Export containers have dates on their individual entries; their outer names
        // do not select a recording day.
        return Ok(None);
    }
    let zone = solstone_core_journal_config::owner_zone(journal);
    let fresh = if let Some(raw) = options.timestamp.as_deref() {
        solstone_core_import::validate_timestamp(raw).ok()
    } else {
        solstone_core_import_sources::image::detected_creation_stamp(
            media,
            media.file_name().and_then(|name| name.to_str()),
            &zone,
        )
    };
    let Some(fresh) = fresh else {
        return Ok(None);
    };
    let hash = solstone_core_import::hash_source(media)
        .map_err(|error| failure("", &format!("{error}\n"), 1))?;
    let stored = solstone_core_import::stored_source_timestamp_for_hash(journal, &hash)
        .map_err(|error| failure("", &format!("{error}\n"), 1))?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    if selected == Some(RegistrySource::Document) && options.timestamp.is_none() {
        return Ok((fresh.day() != stored.day()).then(|| format!(
            "notice: this file's name dates to {}, and a previous import was recorded on {}. the PDF's own dates may select a different day. choose --timestamp to set its date. existing recordings are not moved.\n",
            fresh.day(), stored.day(),
        )));
    }
    let planned = solstone_core_import::source_timestamp_for_preview(journal, &fresh, media)
        .map_err(|error| failure("", &format!("{error}\n"), 1))?;
    Ok(solstone_core_import::force_day_notice(
        &fresh, &stored, &planned,
    ))
}

/// Help and argparse results from the shared importer grammar.
///
/// `None` means the arguments are a command. The desktop adapter runs its
/// supervisor preflight only in that case. Help and grammar errors are the
/// same [`CliRun`] values [`run_cli`] returns, and this function does not probe.
pub fn grammar_outcome_before_preflight(args: &[String]) -> Option<CliRun> {
    json_mode_before_preflight(args).err()
}

/// Resolve the output mode before the desktop supervisor check.
pub fn json_mode_before_preflight(args: &[String]) -> Result<bool, CliRun> {
    match parse_arguments(args) {
        Ok(ParsedCommand::Help) => Err(success(cli_render::HELP.to_owned())),
        Ok(ParsedCommand::RetiredNotesFolder) => Err(notes_folder_retired()),
        Err(arguments) => Err(argparse_error(arguments)),
        Ok(ParsedCommand::Import(options) | ParsedCommand::Sync { options, .. }) => {
            Ok(options.json)
        }
        Ok(
            ParsedCommand::ListImporters { json }
            | ParsedCommand::AudioArchiveStatus { json, .. }
            | ParsedCommand::AudioArchiveMembers { json, .. },
        ) => Ok(json),
        Ok(ParsedCommand::AudioArchive(options)) => Ok(options.json),
        Ok(ParsedCommand::Backends | ParsedCommand::Connect { .. }) => Ok(false),
    }
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
    if matches!(parsed, ParsedCommand::RetiredNotesFolder) {
        return rendered(notes_folder_retired());
    }
    if let Err(refusal) = preflight() {
        return rendered(refusal_run(refusal));
    }

    if let Err(error) = revoke_saved_for_parsed(&parsed, journal_path) {
        return rendered(failure("", &format!("{error}\n"), 1));
    }

    dispatch_parsed(parsed, journal_path, Dispatch::Cli)
}

/// Revoke only the new first-Start permission; execution and retry policy stay
/// with the existing native owner. Parsing/Preview callers never invoke this.
pub(crate) fn revoke_saved_for_args(args: &[String], journal: &Path) -> Result<(), String> {
    let parsed = parse_arguments(args)?;
    revoke_saved_for_parsed(&parsed, journal)
}

fn revoke_saved_for_parsed(parsed: &ParsedCommand, journal: &Path) -> Result<(), String> {
    let ParsedCommand::Import(options) = parsed else {
        return Ok(());
    };
    if options.dry_run {
        return Ok(());
    }
    // Open the root here so a relative or missing journal path is reported as
    // such, not as the restore refusal `begin_target_use` folds it into.
    let root = solstone_core_journal_io::journal_root::JournalRoot::open(journal)
        .map_err(|error| error.to_string())?;
    let _target_use = solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(
        std::sync::Arc::new(root),
    )
    .map_err(|error| error.to_string())?;
    let mut options = options.clone();
    owner_instant(&mut options, journal).map_err(|run| run.stderr)?;
    let Some(media) = options.media.as_deref() else {
        return Ok(());
    };
    // Use the same source/time resolution and read-only record lookup as the
    // eventual binder, including collision-shifted ids and preview snapshots.
    let outcome = resolve(options_ref(&options, media), journal)?;
    let ResolutionOutcome::Resolved { timestamp, .. } = outcome else {
        return Ok(());
    };
    if let Some(id) =
        solstone_core_import::existing_import_record(journal, &timestamp, Path::new(media))
            .map_err(|error| error.to_string())?
    {
        solstone_core_import::saved_start::revoke_saved_start(journal, id.as_str())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) enum Dispatch<'a> {
    Cli,
    Library {
        seams: &'a crate::call::ImportSeams,
        embedded: bool,
    },
}

impl Dispatch<'_> {
    fn embedded(&self) -> bool {
        matches!(self, Self::Library { embedded: true, .. })
    }

    fn seams(&self) -> Option<&crate::call::ImportSeams> {
        match self {
            Self::Library { seams, .. } => Some(seams),
            Self::Cli => None,
        }
    }
}

fn embedded_body_refused() -> CliRun {
    failure("", &format!("{}\n", cli_render::EMBEDDED_BODY_REFUSAL), 1)
}

/// Shape one embedded preview so it cannot save, admit, or publish.
///
/// A file import that is not already a dry run keeps today's digest and zone
/// preview. `--sync SOURCE --save` runs as that source's catalog preview.
/// A non-dry audio archive runs as `--dry-run`. Connect is left for dispatch,
/// which does not start Oura authorization on the embedded entry. Parse
/// failures are the same usage error as execution.
pub(crate) fn library_preview_arguments(
    args: &[String],
    journal: &Path,
) -> Result<Vec<String>, CliRun> {
    let parsed = match parse_arguments(args) {
        Ok(parsed) => parsed,
        Err(message) => return Err(argparse_error(message)),
    };
    match parsed {
        ParsedCommand::Import(options) if !options.dry_run && options.media.is_some() => {
            preview_arguments_with(args, journal, || Ok(()))
                .map(|shaped| shaped.unwrap_or_else(|| args.to_vec()))
        }
        ParsedCommand::Sync { options, .. } if options.save => Ok(without_flag(args, "--save")),
        ParsedCommand::AudioArchive(crate::audio_archive::AudioArchiveDispatchOptions {
            resume: Some(id),
            dry_run: false,
            ..
        }) => Ok(vec![
            "--audio-archive-status".to_owned(),
            id,
            "--json".to_owned(),
        ]),
        ParsedCommand::AudioArchive(options) if !options.dry_run => {
            let mut shaped = Vec::with_capacity(args.len() + 1);
            shaped.push("--dry-run".to_owned());
            shaped.extend_from_slice(args);
            Ok(shaped)
        }
        _ => Ok(args.to_vec()),
    }
}

fn without_flag(args: &[String], flag: &str) -> Vec<String> {
    args.iter()
        .filter(|argument| argument.as_str() != flag)
        .cloned()
        .collect()
}

/// Apple Health and Oura, as the parsed command names them.
///
/// A directory or zip is not classified as Apple Health here. Discovering
/// that export reads the source, which is execution. `--source oura` on a
/// file import is not a sync and is not this enum; resolution refuses it
/// with the existing sync remedy and does not call a body writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyAdmission {
    /// Not `--source apple_health`, `--connect oura`, or `--sync oura`.
    NotBody,
    /// Apple Health import, Oura connect, or an Oura sync.
    Held,
}

/// Parsed shape of one importer invocation.
///
/// Built by the same grammar as execution. It does not open a journal, probe
/// the supervisor, or read source bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgumentClass {
    /// Backend of `--sync`. `None` for every other command.
    pub sync_source: Option<String>,
    /// Held ordinary Body entry, stored gated Oura, or neither.
    pub body: BodyAdmission,
}

/// Classify `args` with the importer grammar.
///
/// The class is how a caller identifies the sync backend and separates a held
/// Body entry from stored gated Oura without scanning flags itself. Acceptance
/// matches [`admit_arguments`]. An import with no media path is rejected with
/// the same requirement execution reports.
pub fn classify_arguments(args: &[String]) -> Result<ArgumentClass, String> {
    let parsed = parse_arguments(args)?;
    if matches!(&parsed, ParsedCommand::Import(options) if options.media.is_none()) {
        return Err("the following arguments are required: media".to_owned());
    }
    Ok(classify_parsed(&parsed))
}

fn classify_parsed(parsed: &ParsedCommand) -> ArgumentClass {
    match parsed {
        ParsedCommand::Sync { backend, .. } => ArgumentClass {
            sync_source: Some(backend.clone()),
            body: if backend == "oura" {
                BodyAdmission::Held
            } else {
                BodyAdmission::NotBody
            },
        },
        ParsedCommand::Connect { backend } if backend == "oura" => ArgumentClass {
            sync_source: None,
            body: BodyAdmission::Held,
        },
        ParsedCommand::Import(options) if options.source.as_deref() == Some("apple_health") => {
            ArgumentClass {
                sync_source: None,
                body: BodyAdmission::Held,
            }
        }
        _ => ArgumentClass {
            sync_source: None,
            body: BodyAdmission::NotBody,
        },
    }
}

/// Whether `args` is one importer invocation this grammar accepts.
///
/// The check parses only. It does not open a journal, probe the supervisor,
/// or read source bytes. Acceptance is not a preview, not publication, and
/// not approval of Apple Health or Oura. An import with no media path is
/// rejected with the same requirement execution reports. Call
/// [`classify_arguments`] for the sync source and Body class.
pub fn admit_arguments(args: &[String]) -> Result<(), String> {
    classify_arguments(args).map(|_| ())
}

pub(crate) fn run_library(
    args: &[String],
    journal_path: &Path,
    seams: &crate::call::ImportSeams,
    embedded: bool,
) -> (CliRun, Option<String>) {
    let started_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let parsed = match parse_arguments(args) {
        Ok(ParsedCommand::Help) => return (success(cli_render::HELP.to_owned()), None),
        Ok(parsed) => parsed,
        Err(arguments) => return (argparse_error(arguments), None),
    };
    if !solstone_core_import::work_continues() {
        return (failure("", "import stopped\n", 1), None);
    }
    let (run, import_id) =
        match dispatch_parsed(parsed, journal_path, Dispatch::Library { seams, embedded }) {
            CliOutcome::Rendered(run) => (run, None),
            CliOutcome::Imported { run, import_id } => (run, Some(import_id)),
            CliOutcome::Registry(_) => {
                unreachable!("library dispatch executes registry sources")
            }
        };
    if run.exit_code == 0
        && let Some(import_id) = &import_id
    {
        crate::registry::record_finished_import(journal_path, import_id, started_ms);
    }
    (run, import_id)
}

fn dispatch_parsed(
    parsed: ParsedCommand,
    journal_path: &Path,
    dispatch: Dispatch<'_>,
) -> CliOutcome {
    match parsed {
        ParsedCommand::Help => rendered(success(cli_render::HELP.to_owned())),
        ParsedCommand::ListImporters { json } => rendered(success(cli_render::importers(json))),
        ParsedCommand::Backends => rendered(success(cli_render::backends())),
        ParsedCommand::Connect { backend } => {
            if dispatch.embedded() && backend == "oura" {
                return rendered(embedded_body_refused());
            }
            rendered(run_connect(&backend, journal_path))
        }
        ParsedCommand::RetiredNotesFolder => rendered(notes_folder_retired()),
        ParsedCommand::Sync { backend, options } => {
            if dispatch.embedded() && backend == "oura" {
                return rendered(embedded_body_refused());
            }
            rendered(run_sync(&backend, &options, journal_path))
        }
        ParsedCommand::AudioArchive(options) => rendered(crate::audio_archive::run_audio_archive(
            options,
            journal_path,
        )),
        ParsedCommand::AudioArchiveStatus { run_id, json } => rendered(
            crate::audio_archive::run_audio_archive_status(&run_id, json, journal_path),
        ),
        ParsedCommand::AudioArchiveMembers {
            run_id,
            offset,
            limit,
            json,
        } => rendered(crate::audio_archive::run_audio_archive_members(
            &run_id,
            offset,
            limit,
            json,
            journal_path,
        )),
        ParsedCommand::Import(options) => run_import(options, journal_path, dispatch),
    }
}

fn notes_folder_retired() -> CliRun {
    failure(
        "",
        &format!("{}\n", cli_render::NOTES_FOLDER_IMPORT_RETIRED),
        1,
    )
}

fn refusal_run(refusal: SupervisorRefusal) -> CliRun {
    match refusal {
        SupervisorRefusal::SpawnedUnavailable => failure("", "", 75),
        SupervisorRefusal::Unavailable => failure("", &format!("{SUPERVISOR_MESSAGE}\n"), 1),
    }
}

fn run_connect(backend: &str, journal_path: &Path) -> CliRun {
    match backend {
        "plaud" => return run_plaud_connect(journal_path),
        "oura" => {}
        _ => {
            return failure(
                "",
                &format!("Unknown connect backend: {backend}\nConnectable backends: oura, plaud\n"),
                1,
            );
        }
    }
    let connect = |app| {
        connect_oura(&OuraConnectRequest {
            journal_root: journal_path.to_path_buf(),
            timeout_seconds: 300,
            app,
        })
    };
    let result = match connect(None) {
        Err(error) if needs_oura_app(&error) => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return failure("", &crate::oura_app_question::app_needed_words(), 1);
            }
            let asked = crate::oura_app_question::ask_oura_app(
                &mut std::io::stdin().lock(),
                &mut std::io::stdout(),
            );
            let Some(app) = asked else {
                return failure(
                    "",
                    "Oura needs both the client ID and the client secret, so nothing was saved.\n",
                    1,
                );
            };
            connect(Some(app))
        }
        other => other,
    };
    match result {
        Ok(outcome) => success(format!(
            "Oura is connected, and the connection is saved in this journal's settings.\npermissions asked for: {}\n",
            outcome.report.scopes().join(" ")
        )),
        Err(error) => failure(
            "",
            &crate::oura_app_question::connect_failed_words(&error),
            1,
        ),
    }
}

fn run_sync(backend: &str, options: &Options, journal_path: &Path) -> CliRun {
    match backend {
        "oura" => crate::body_commands::sync_oura(&body_command(options), journal_path),
        "plaud" => run_plaud_sync(journal_path, options),
        "granola" => {
            let result = if options.save && !options.dry_run {
                solstone_core_import::granola::sync(journal_path)
                    .and_then(|()| solstone_core_import::granola::status(journal_path))
            } else {
                solstone_core_import::granola::status(journal_path)
            };
            match result {
                Ok(status) => success(format!("{status}\n")),
                Err(error) => failure("", &format!("{error}\n"), 1),
            }
        }
        _ => failure(
            "",
            &format!(
                "Unknown sync backend: {backend}\nAvailable backends: {}\n",
                cli_render::sync_backends().collect::<Vec<_>>().join(", ")
            ),
            1,
        ),
    }
}

fn run_import(options: Options, journal_path: &Path, dispatch: Dispatch<'_>) -> CliOutcome {
    // A page start's import is checked before anything else, so a start aimed at
    // an import that already ran never changes it.
    let page_import = options.import_id.clone();
    // A dry run (the preview the desktop command runs before every import)
    // names the same import, and writes and removes nothing.
    let dry_run = options.dry_run;
    if let Some(id) = page_import.as_deref() {
        // Only an Apple Health card's save names its import. Anything else that
        // names one is refused here, and nothing is written or removed.
        let apple_save =
            options.source.as_deref() == Some("apple_health") && options.media.is_some();
        if !apple_save {
            return rendered(failure("", crate::body_commands::CARD_ONLY, 2));
        }
        if let Err(run) = crate::body_commands::accept_page_import(
            journal_path,
            id,
            options.media.as_deref().unwrap_or_default(),
        ) {
            return rendered(run);
        }
    }
    let outcome = run_import_accepted(options, journal_path, dispatch);
    if let Some(id) = page_import.as_deref().filter(|_| !dry_run) {
        let failed = match &outcome {
            CliOutcome::Rendered(run) | CliOutcome::Imported { run, .. } if run.exit_code != 0 => {
                Some(run.stderr.trim_end())
            }
            _ => None,
        };
        crate::body_commands::settle_page_import(journal_path, id, failed);
    }
    outcome
}

fn run_import_accepted(
    mut options: Options,
    journal_path: &Path,
    dispatch: Dispatch<'_>,
) -> CliOutcome {
    let owner_instant = match owner_instant(&mut options, journal_path) {
        Ok(instant) => instant,
        Err(run) => return rendered(run),
    };
    let Some(media) = options.media.as_deref() else {
        return rendered(argparse_error(
            "the following arguments are required: media".to_owned(),
        ));
    };
    if options.preview_zone.as_deref().is_some_and(|zone| {
        Some(zone) != solstone_core_journal_config::owner_zone(journal_path).iana_name()
    }) {
        return rendered(failure(
            "",
            "the time zone changed after preview; preview again before importing\n",
            1,
        ));
    }
    if let Some(digest) = &options.snapshot_digest
        && let Err(error) = solstone_core_import::snapshot::verify(Path::new(media), digest)
    {
        return rendered(failure("", &format!("{error}\n"), 1));
    }
    if let Some(token) = &options.preview_token {
        let timestamp = options
            .preview_stamp
            .as_deref()
            .or(options.timestamp.as_deref())
            .unwrap_or_default();
        if let Err(error) = solstone_core_import::snapshot::verify_preview(
            journal_path,
            Path::new(media),
            token,
            timestamp,
            options.force,
            options.setting.as_deref(),
            options.source.as_deref(),
        ) {
            return rendered(failure("", &format!("{error}\n"), 1));
        }
    }
    if options.facet.is_some() && options.no_facet {
        return rendered(argparse_error(
            "argument --no-facet: not allowed with argument --facet".to_owned(),
        ));
    }
    if let Some(token) = &options.preview_token
        && let Err(error) = solstone_core_import::snapshot::verify_preview_body_facet(
            journal_path,
            Path::new(media),
            token,
            import_facet(&options).preview_value().as_ref(),
        )
    {
        return rendered(failure("", &format!("{error}\n"), 1));
    }
    if let Some(facet) = options.facet.as_deref()
        && let Err(error) = require_enabled_facet(facet, journal_path)
    {
        return rendered(failure("", &format!("{error}\n"), 1));
    }
    let outcome = match resolve(options_ref(&options, media), journal_path) {
        Ok(outcome) => outcome,
        Err(error) => return rendered(failure("", &format!("{error}\n"), 1)),
    };
    let strava = matches!(
        outcome,
        ResolutionOutcome::Resolved {
            source: ResolvedSource::Registry(RegistrySource::Strava),
            ..
        }
    );
    if options.facet.is_some()
        && !strava
        && matches!(
            outcome,
            ResolutionOutcome::Resolved {
                source: ResolvedSource::Registry(_),
                ..
            }
        )
    {
        return rendered(failure(
            "",
            "--facet only applies to an audio file, a plain text file or a body import, so nothing was imported; import this one without it\n",
            1,
        ));
    }
    if options.no_facet && !strava && !matches!(outcome, ResolutionOutcome::RouteAppleHealth) {
        return rendered(failure(
            "",
            "--no-facet only applies to a body import, so nothing was imported; import this one without it\n",
            1,
        ));
    }
    match outcome {
        ResolutionOutcome::RouteAppleHealth => {
            if dispatch.embedded() {
                rendered(embedded_body_refused())
            } else {
                rendered(if options.dry_run {
                    crate::body_commands::preview_apple(
                        media,
                        journal_path,
                        &import_facet(&options),
                        options.json,
                    )
                } else {
                    crate::body_commands::save_apple(media, &body_command(&options), journal_path)
                })
            }
        }
        ResolutionOutcome::Skipped {
            reason: solstone_core_import::SkipReason::TimestampRequired,
            detected_timestamp: Some(timestamp),
        } => rendered(failure(
            "",
            &cli_render::timestamp_confirmation(timestamp.as_str()),
            1,
        )),
        ResolutionOutcome::Skipped { reason, .. } => {
            if options.dry_run {
                rendered(success(
                    solstone_core_import::size::ImportSize {
                        skipped: 1,
                        ..Default::default()
                    }
                    .render(options.json),
                ))
            } else {
                rendered(success(cli_render::resolution_skipped(reason)))
            }
        }
        ResolutionOutcome::Resolved {
            source: ResolvedSource::GenericAudio,
            timestamp,
            ..
        } => {
            let timestamp = with_owner_instant(timestamp, owner_instant);
            let (run, import_id) = run_audio(media, &options, journal_path, &timestamp, true);
            imported(run, &import_id, options.dry_run)
        }
        ResolutionOutcome::Resolved {
            source: ResolvedSource::GenericText,
            timestamp,
            stream,
        } => {
            let timestamp = with_owner_instant(timestamp, owner_instant);
            match dispatch.seams() {
                Some(seams) => {
                    let (run, import_id) = run_text(
                        media,
                        &options,
                        journal_path,
                        &timestamp,
                        &stream,
                        seams.text.as_ref(),
                    );
                    seams.text.finish();
                    imported(run, &import_id, options.dry_run)
                }
                None => {
                    let wire = solstone_core_import::text::SystemWireClient::sibling();
                    let (run, import_id) =
                        run_text(media, &options, journal_path, &timestamp, &stream, &wire);
                    wire.finish();
                    imported(run, &import_id, options.dry_run)
                }
            }
        }
        ResolutionOutcome::Resolved {
            source: ResolvedSource::Registry(source),
            timestamp,
            ..
        } => {
            let (facet, listed_moves_shown) =
                asked_import_facet(&options, source, media, journal_path);
            let registry_dispatch = RegistryDispatch {
                media: PathBuf::from(media),
                source,
                timestamp: timestamp.as_str().to_owned(),
                dry_run: options.dry_run,
                json: options.json,
                force: options.force,
                owner_timestamp: options.timestamp.clone(),
                fallback_timestamp: options.fallback_timestamp.clone(),
                facet,
                listed_moves_shown,
            };
            match dispatch.seams() {
                None => CliOutcome::Registry(registry_dispatch),
                Some(seams) => {
                    let reindex = seams.reindex.as_ref();
                    let source_seams = crate::registry::SourceSeams {
                        pdf: seams.pdf.as_ref(),
                        model: seams.model.as_ref(),
                        image: seams.image.as_ref(),
                        reindex,
                    };
                    let (run, import_id) = crate::registry::run_with_seams(
                        registry_dispatch,
                        journal_path,
                        &source_seams,
                    );
                    match import_id {
                        Some(import_id) if !options.dry_run => {
                            CliOutcome::Imported { run, import_id }
                        }
                        _ => CliOutcome::Rendered(run),
                    }
                }
            }
        }
    }
}

/// The facet a run was given, or the owner's answer when it's a saving Strava
/// run at a terminal with no facet named and a question still open. Anything
/// else goes on unattended. No lock or sweep has started yet, so a terminal
/// left waiting holds nothing.
fn asked_import_facet(
    options: &Options,
    source: RegistrySource,
    media: &str,
    journal: &Path,
) -> (ImportFacet, Option<usize>) {
    let given = import_facet(options);
    if given != ImportFacet::Unattended
        || source != RegistrySource::Strava
        || options.dry_run
        || options.scheduled
        || options.json
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        return (given, options.listed_moves_shown);
    }
    crate::body_facet_question::ask_strava_facet(
        journal,
        Path::new(media),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
}

fn body_command(options: &Options) -> crate::body_commands::BodyCommand<'_> {
    crate::body_commands::BodyCommand {
        import_id: options.import_id.as_deref(),
        facet: import_facet(options),
        listed_moves_shown: options.listed_moves_shown,
        scheduled: options.scheduled,
        json: options.json,
        confirm: options.confirm_body_save,
        date_from: options.date_from.as_deref(),
        date_to: options.date_to.as_deref(),
        window_days: options.window_days,
        force: options.force,
        save: options.save,
    }
}

/// The facet a run was given on its command line.
fn import_facet(options: &Options) -> ImportFacet {
    match (&options.facet, options.no_facet) {
        (Some(facet), _) => ImportFacet::Facet(facet.clone()),
        (None, true) => ImportFacet::None,
        (None, false) => ImportFacet::Unattended,
    }
}

/// An import filed under a facet the journal would not route to would land somewhere the
/// owner did not choose, so it never starts.
/// What a refused facet's words always say.
pub(crate) const FACET_REFUSAL_MARKER: &str = "is not one of your unmuted facets";

pub(crate) fn require_enabled_facet(facet: &str, journal: &Path) -> Result<(), String> {
    let inventory = solstone_core_facets::observe_declared_facet_inventory(journal)
        .map_err(|error| format!("could not read your facets, so nothing was imported: {error}"))?;
    if inventory.enabled.iter().any(|enabled| enabled == facet) {
        return Ok(());
    }
    Err(format!(
        "\"{facet}\" {FACET_REFUSAL_MARKER}, so nothing was imported. your unmuted facets: {}",
        inventory.enabled.join(", ")
    ))
}

/// An owner's `--timestamp` may name the instant itself, with its offset, which is the only
/// way to say the second occurrence of a repeated hour. It becomes the journal's own
/// `YYYYMMDD_HHMMSS` here, so every importer reads one form, and the instant is returned for
/// the imports that place elapsed time. A `YYYYMMDD_HHMMSS` value is left as it was.
fn owner_instant(options: &mut Options, journal: &Path) -> Result<Option<Timestamp>, CliRun> {
    let Some(raw) = options.timestamp.as_deref() else {
        return Ok(None);
    };
    if solstone_core_import::validate_timestamp(raw)
        != Err(solstone_core_import::TimestampError::Shape)
    {
        return Ok(None);
    }
    let zone = solstone_core_journal_config::owner_zone(journal);
    let Some(stamp) = solstone_core_import::owner_instant(raw, &zone) else {
        return Err(failure(
            "",
            "timestamp must be YYYYMMDD_HHMMSS, or a date and time with its offset, such as 2025-11-02T01:30:00-07:00\n",
            1,
        ));
    };
    options.timestamp = Some(stamp.as_str().to_owned());
    Ok(stamp.source_instant())
}

fn with_owner_instant(
    timestamp: solstone_core_import::Timestamp,
    instant: Option<Timestamp>,
) -> solstone_core_import::Timestamp {
    match instant {
        Some(instant) => timestamp.with_source_instant(instant),
        None => timestamp,
    }
}

fn rendered(run: CliRun) -> CliOutcome {
    CliOutcome::Rendered(run)
}

fn imported(run: CliRun, import_id: &str, dry_run: bool) -> CliOutcome {
    if dry_run {
        rendered(run)
    } else {
        CliOutcome::Imported {
            run,
            import_id: import_id.to_owned(),
        }
    }
}

/// Current-thread runtime for generic audio import, including the processing wait.
///
/// The calling thread must not already have a Tokio runtime. Nesting one
/// panics inside the runtime constructor, so this returns an error instead.
/// The embedded entry runs audio on a fresh thread that has no runtime.
/// Desktop `execute_desktop` uses the caller's thread and has the same rule.
pub fn audio_import_runtime() -> Result<tokio::runtime::Runtime, String> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(
            "generic audio import builds its runtime on a thread that does not already have one"
                .to_owned(),
        );
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| error.to_string())
}

/// The importer waits 30 seconds plus twice the longest segment's audio for a
/// segment to finish: room for a computer that transcribes at half real time.
const STALL_AUDIO_FACTOR: f64 = 2.0;

fn run_audio(
    media: &str,
    options: &Options,
    journal_path: &Path,
    source_stamp: &solstone_core_import::Timestamp,
    wait_for_processing: bool,
) -> (CliRun, String) {
    let timestamp = source_stamp.as_str();
    if options.dry_run {
        let semantic = Ok::<_, solstone_core_import::TimestampError>(source_stamp.clone())
            .map_err(|error| error.to_string())
            .and_then(|requested| {
                solstone_core_import::source_timestamp_for_preview(
                    journal_path,
                    &requested,
                    Path::new(media),
                )
                .map_err(|error| error.to_string())
            });
        let semantic = match semantic {
            Ok(value) => value,
            Err(error) => return (failure("", &format!("{error}\n"), 1), timestamp.to_owned()),
        };
        let base_timestamp =
            match jiff::civil::DateTime::strptime("%Y%m%d_%H%M%S", semantic.as_str()) {
                Ok(value) => value,
                Err(error) => return (failure("", &format!("{error}\n"), 1), timestamp.to_owned()),
            };
        let request = AudioImportRequest {
            source_instant: semantic.source_instant(),
            source_media: PathBuf::from(media),
            journal_root: journal_path.to_path_buf(),
            day: solstone_core_time::day_key(base_timestamp.date()),
            base_timestamp,
            import_id: timestamp.to_owned(),
            stream: "import.audio".to_owned(),
            facet: options.facet.clone(),
            setting: options.setting.clone(),
            wait_for_processing: false,
            stall_timeout: Duration::from_secs(30),
            stall_audio_factor: STALL_AUDIO_FACTOR,
            poll_interval: Duration::from_millis(250),
        };
        return (
            match crate::audio::preview(&request) {
                Ok(size) => success(size.render(options.json)),
                Err(error) => failure("", &format!("{error}\n"), 1),
            },
            timestamp.to_owned(),
        );
    }
    let requested = match Ok::<_, solstone_core_import::TimestampError>(source_stamp.clone()) {
        Ok(value) => value,
        Err(error) => {
            return (failure("", &format!("{error}\n"), 1), timestamp.to_owned());
        }
    };
    // The directory id can move off `timestamp`. Chronicle day and clock stay on
    // the source timestamp, including when the id crosses midnight.
    let bound = match solstone_core_import::bind_import_record(
        journal_path,
        &requested,
        Some(Path::new(media)),
    ) {
        Ok(bound) => bound,
        Err(error) => {
            return (
                failure("", &format!("audio import failed: {error}\n"), 1),
                timestamp.to_owned(),
            );
        }
    };
    let import_id = bound.import_id.as_str().to_owned();
    if let Err(error) =
        solstone_core_import::originals::capture_choice(journal_path, &import_id, "audio")
    {
        return (
            failure("", &format!("audio import failed: {error}\n"), 1),
            import_id,
        );
    }
    let source_timestamp = bound.source_timestamp;
    let base_timestamp =
        match jiff::civil::DateTime::strptime("%Y%m%d_%H%M%S", source_timestamp.as_str()) {
            Ok(value) => value,
            Err(_) => {
                return (
                    failure("", "timestamp must be YYYYMMDD_HHMMSS format\n", 1),
                    import_id,
                );
            }
        };
    let runtime = match audio_import_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            return (
                failure("", &format!("audio import runtime failed: {error}\n"), 1),
                import_id,
            );
        }
    };
    // Everything that can fail before the producer starts has now run, so an admitted
    // attempt always has a producer behind it. Admitting above this point would leave a
    // Running row for the full wall-clock bound on an invocation that failed instantly.
    if let Some(message) =
        solstone_core_import::refuse_if_live_running(journal_path, &import_id, "audio")
    {
        return (failure("", &format!("{message}\n"), 1), import_id);
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
        &import_id,
        started_at_ms,
        None,
    ) {
        Ok(facts) => facts,
        Err(error) => {
            return (
                failure("", &format!("audio import failed: {error}\n"), 1),
                import_id,
            );
        }
    };
    let request = AudioImportRequest {
        source_instant: source_timestamp.source_instant(),
        source_media: PathBuf::from(media),
        journal_root: journal_path.to_path_buf(),
        day: source_timestamp.day().to_owned(),
        base_timestamp,
        import_id: import_id.clone(),
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
        stall_audio_factor: STALL_AUDIO_FACTOR,
        poll_interval: Duration::from_millis(250),
    };
    let outcome = runtime.block_on(import_audio(request));
    finish_audio_attempt(journal_path, &import_id, attempt.generation, &outcome);
    // Only an import that finished cleanly is recorded: a file with a part left out, or a
    // failed or stalled segment, should be able to import again.
    let manifest = match &outcome {
        Ok(imported)
            if imported.writes_dedupe_manifest()
                && imported.created().processing.failed_segments.is_empty()
                && imported.created().processing.stalled_segments.is_empty() =>
        {
            Some(write_audio_manifest(
                journal_path,
                &import_id,
                Path::new(media),
                imported.created(),
            ))
        }
        _ => None,
    };
    if matches!(manifest, Some(Ok(())))
        || outcome.as_ref().is_ok_and(|imported| {
            imported.writes_dedupe_manifest()
                && imported.created().processing.failed_segments.is_empty()
        })
    {
        let cleanup = (|| -> Result<(), String> {
            if solstone_core_import::originals::captured_choice(journal_path, &import_id).is_none()
            {
                return Ok(());
            }
            let record = crate::audio::read_audio_import_record(journal_path, &import_id)
                .map_err(|error| error.to_string())?
                .ok_or("missing audio import record")?;
            let parts = record
                .created_segments
                .into_iter()
                .map(|part| {
                    (
                        part.file_path,
                        (part.start_offset_seconds * 1_000.0).round() as i64,
                    )
                })
                .collect::<Vec<_>>();
            solstone_core_import::audio_originals::plan(
                journal_path,
                &import_id,
                attempt.generation,
                Path::new(media),
                &parts,
            )?;
            solstone_core_import::audio_originals::finish(journal_path, &import_id)
        })();
        if let Err(error) = cleanup {
            log::warn!("imported audio cleanup {import_id}: {error}");
        }
    }
    let run = audio_import_cli_run(outcome);
    let run = match manifest {
        Some(Err(error)) if run.exit_code == 0 => failure(
            &run.stdout,
            &format!(
                "audio import saved to your journal, but this file couldn't be marked as imported, so importing it again would add it a second time: {error}\n"
            ),
            1,
        ),
        _ => run,
    };
    (run, import_id)
}

/// Record a finished audio import by its source hash, the record the already-imported check
/// and the audio sync both read. Without it the same recording imports a second time.
fn write_audio_manifest(
    journal_path: &Path,
    import_id: &str,
    media: &Path,
    created: &crate::audio::AudioImportComplete,
) -> Result<(), String> {
    let source_hash =
        solstone_core_import::hash_source(media).map_err(|error| error.to_string())?;
    let mut days_affected = created
        .segments
        .iter()
        .map(|segment| segment.day.clone())
        .collect::<Vec<_>>();
    days_affected.sort();
    days_affected.dedup();
    let files_created = created
        .files_created
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    solstone_core_import::write_manifest(&solstone_core_import::ManifestWriteRequest {
        journal_root: journal_path,
        import_id,
        source_type: "audio",
        source_hash: &source_hash,
        entry_count: u64::try_from(created.segments.len()).unwrap_or(u64::MAX),
        days_affected: &days_affected,
        files_created: &files_created,
        imported_via: "native",
        link_id: None,
        observer_handle: None,
        raw_retention: None,
        span: None,
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn run_text(
    media: &str,
    options: &Options,
    journal_path: &Path,
    timestamp: &solstone_core_import::Timestamp,
    stream: &str,
    wire: &dyn solstone_core_import::text::WireClient,
) -> (CliRun, String) {
    if options.dry_run {
        let semantic = match solstone_core_import::source_timestamp_for_preview(
            journal_path,
            timestamp,
            Path::new(media),
        ) {
            Ok(value) => value,
            Err(error) => {
                return (
                    failure("", &format!("{error}\n"), 1),
                    timestamp.as_str().to_owned(),
                );
            }
        };
        let timestamp = &semantic;
        let day_dir = journal_path.join("chronicle").join(timestamp.day());
        return (
            match solstone_core_import::text::preview(
                Path::new(media),
                &day_dir,
                &timestamp.clock(),
                stream,
                timestamp.source_instant(),
            ) {
                Ok(size) => success(size.render(options.json)),
                Err(error) => failure("", &format!("{error}\n"), 1),
            },
            timestamp.as_str().to_owned(),
        );
    }
    let bound = match solstone_core_import::bind_import_record(
        journal_path,
        timestamp,
        Some(Path::new(media)),
    ) {
        Ok(bound) => bound,
        Err(error) => {
            return (
                failure("", &format!("text import failed: {error}\n"), 1),
                timestamp.as_str().to_owned(),
            );
        }
    };
    let import_id = bound.import_id.as_str().to_owned();
    let source_timestamp = bound.source_timestamp;
    if let Some(error) = refuse_if_live_running(journal_path, &import_id, "text") {
        return (failure("", &format!("{error}\n"), 1), import_id);
    }
    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // No source hint: `admit_running_attempt` persists one, the web start replays it as
    // `--source <hint>`, and `resolve` refuses any name `RegistrySource` does not know --
    // which has no text variant. The projection derives "text" from the publication's
    // `import.text` stream prefix instead, so the hint buys nothing and would break restart.
    let generation = match admit_running_attempt(journal_path, &import_id, started_at_ms, None) {
        Ok(facts) => facts.generation,
        Err(error) => {
            return (failure("", &format!("{error}\n"), 1), import_id);
        }
    };
    let day_dir = journal_path.join("chronicle").join(source_timestamp.day());
    if let Err(error) = fs::create_dir_all(&day_dir) {
        let _ = finish_import_attempt(
            journal_path,
            &import_id,
            generation,
            "text",
            ImportTerminalInput::Failed(&[]),
        );
        return (failure("", &format!("{error}\n"), 1), import_id);
    }
    // process_transcript's start_time is a transcript clock (`HH:MM:SS`), not
    // the stamp half (`HHMMSS`). Convert at this seam; do not teach the
    // transcript parser a second format. The clock is the source time, not a
    // shifted bookkeeping id.
    let clock = source_timestamp.clock();
    let outcome = solstone_core_import::text::process_transcript_with_wire(
        Path::new(media),
        &day_dir,
        &clock,
        &import_id,
        stream,
        options.facet.as_deref(),
        options.setting.as_deref(),
        wire,
        source_timestamp.source_instant(),
    );
    let input = match &outcome {
        TextImportOutcome::Success(work) => ImportTerminalInput::Success(&work.created),
        TextImportOutcome::Failed { created, .. } => ImportTerminalInput::Failed(&created.created),
    };
    let finish = finish_import_attempt(journal_path, &import_id, generation, "text", input);
    if let Err(error) = finish {
        return (failure("", &format!("{error}\n"), 1), import_id);
    }
    let run = match outcome {
        // The terminal record is the authority on whether publication held: a failed
        // publication is recorded as unconfirmed, not reported back as an error.
        TextImportOutcome::Success(_)
            if solstone_core_import::project_import_result(journal_path, &import_id).status
                != solstone_core_import::ProjectionStatus::Success =>
        {
            failure(
                "",
                "text import saved to your journal, but it could not confirm the entries are ready to search; run it again\n",
                1,
            )
        }
        TextImportOutcome::Success(work) => success(cli_render::generic_text_complete(
            work.created.len(),
            work.untimed,
        )),
        TextImportOutcome::Failed { error, .. } => failure("", &format!("{error}\n"), 1),
    };
    (run, import_id)
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
        fallback_timestamp: options.fallback_timestamp.as_deref(),
    }
}

fn resolve(
    mut options: ResolutionOptions<'_>,
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
        && !solstone_core_import_sources::apple_export::detect_apple_source(options.media).map_err(
            |_| "could not inspect Apple Health export; retry with a valid export".to_owned(),
        )?
    {
        // An export dropped with no source named is recognised by its content, in registry
        // order, as each source's own detector reads it.
        options.source = Some(classify_export(options.media).ok_or_else(|| {
            "could not tell what kind of export this is; name it with --source, for example --source chatgpt"
                .to_owned()
        })?);
    }
    if options.source.is_none()
        && is_generic_media(options.media)
        && !options.force
        && let Some(manifest) = generic_manifest_summary(journal_path, options.media)?
        && manifest.entry_count > 0
    {
        return Ok(ResolutionOutcome::Skipped {
            reason: solstone_core_import::SkipReason::AlreadyImported,
            detected_timestamp: None,
        });
    }
    let zone = solstone_core_journal_config::owner_zone(journal_path);
    let mut seams = ResolutionSeams {
        apple_detector: solstone_core_import_sources::apple_export::detect_apple_source,
        // The source crate does not depend on this crate. The host pre-classifies
        // through solstone_core_import_sources::registry::claims in classify_export.
        // The resolver seam stays no_registry_claim so selection is not applied twice.
        claims: no_registry_claim,
        deterministic_detector: |path: &Path, name: Option<&str>| {
            solstone_core_import_sources::image::detected_creation_stamp(path, name, &zone)
                .map(solstone_core_import::DetectedTimestamp::new)
        },
        model_detector: unavailable_model_timestamp,
        // Generic manifest deduplication is performed above with the resolved
        // journal root. The resolver retains this seam for its library callers.
        manifest_lookup: no_manifest_match,
        generated_timestamp: || {
            solstone_core_import::validate_timestamp(
                &Timestamp::now()
                    .to_zoned_datetime(zone.clone())
                    .strftime("%Y%m%d_%H%M%S")
                    .to_string(),
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

fn lowercase_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
}

fn classify_export(path: &Path) -> Option<&'static str> {
    solstone_core_import::detect::ORDERED_FILE_IMPORTER_NAMES
        .iter()
        .copied()
        .find(|name| {
            solstone_core_import::RegistrySource::from_name(name).is_some_and(|source| {
                solstone_core_import_sources::registry::claims(source, path).unwrap_or(false)
            })
        })
}

fn is_generic_audio_extension(extension: &str) -> bool {
    GENERIC_AUDIO_EXTENSIONS.contains(&extension)
}

fn requires_registry_classification(path: &Path) -> bool {
    let Some(extension) = lowercase_extension(path) else {
        return true;
    };
    !(is_generic_audio_extension(extension.as_str())
        || solstone_core_import::detect::is_generic_text_extension(extension.as_str()))
}

fn is_generic_media(path: &Path) -> bool {
    lowercase_extension(path).is_some_and(|extension| {
        is_generic_audio_extension(extension.as_str())
            || solstone_core_import::detect::is_generic_text_extension(extension.as_str())
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

fn run_plaud_connect(journal_path: &Path) -> CliRun {
    use std::io::{BufRead, Write};
    use std::sync::mpsc;

    use solstone_core_import::plaud::{self, PlaudSignInOutcome};

    let attempt = match plaud::begin_sign_in(None) {
        Ok(attempt) => attempt,
        Err(error) => {
            return failure(
                "",
                &format!(
                    "{}\n",
                    solstone_core_cli_copy::strings(
                        "journal.cli.importer.plaud.connect.failed",
                        [("detail", &error.to_string())],
                    )
                ),
                1,
            );
        }
    };

    if let Ok(invocation) = solstone_core_oauth::plan_browser(
        solstone_core_oauth::current_target_os(),
        &attempt.authorize_url,
    ) {
        let _ = solstone_core_oauth::execute_browser_invocation(&invocation);
    }

    let mut out = std::io::stdout();
    let _ = writeln!(
        out,
        "{}\n{}\n",
        solstone_core_cli_copy::text("journal.cli.importer.plaud.connect.open"),
        attempt.authorize_url,
    );
    let _ = writeln!(
        out,
        "{}",
        solstone_core_cli_copy::text("journal.cli.importer.plaud.connect.waiting"),
    );
    let _ = out.flush();

    let (tx, rx) = mpsc::channel();

    let tx_loopback = tx.clone();
    let attempt_loopback = attempt.clone();
    let journal_loopback = journal_path.to_path_buf();
    std::thread::spawn(move || {
        match plaud::finish_sign_in(
            &journal_loopback,
            &attempt_loopback,
            None,
            Duration::from_secs(300),
        ) {
            Ok(finished) => {
                let _ = tx_loopback.send(Ok(finished));
            }
            Err(PlaudSignInOutcome::Cancelled) => {}
            Err(outcome) => {
                let _ = tx_loopback.send(Err(outcome));
            }
        }
    });

    let tx_stdin = tx;
    let attempt_stdin = attempt.clone();
    let journal_stdin = journal_path.to_path_buf();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if attempt_stdin.gate.is_claimed() {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match plaud::finish_sign_in(
                &journal_stdin,
                &attempt_stdin,
                Some(trimmed),
                Duration::from_secs(30),
            ) {
                Ok(finished) => {
                    let _ = tx_stdin.send(Ok(finished));
                    break;
                }
                Err(PlaudSignInOutcome::CallbackInvalid) => {
                    if attempt_stdin.gate.is_claimed() {
                        break;
                    }
                    eprintln!(
                        "{}",
                        solstone_core_cli_copy::text(
                            "journal.cli.importer.plaud.connect.retry.url"
                        )
                    );
                }
                Err(PlaudSignInOutcome::Cancelled) => break,
                Err(outcome) => {
                    let _ = tx_stdin.send(Err(outcome));
                    break;
                }
            }
        }
    });

    let result = rx.recv().unwrap_or(Err(PlaudSignInOutcome::Cancelled));
    match result {
        Ok(finished) => {
            let headline = match finished.account_label.as_deref() {
                Some(account) => solstone_core_cli_copy::strings(
                    "journal.cli.importer.plaud.connect.done.account",
                    [("account", account)],
                ),
                None => solstone_core_cli_copy::text("journal.cli.importer.plaud.connect.done"),
            };
            let next = solstone_core_cli_copy::text("journal.cli.importer.plaud.connect.next");
            success(format!("{headline}\n{next}\n"))
        }
        Err(outcome) => {
            plaud::cancel_active_attempt();
            let detail = match outcome {
                PlaudSignInOutcome::Denied => "sign-in was not approved",
                PlaudSignInOutcome::CallbackInvalid => "the sign-in address was invalid",
                PlaudSignInOutcome::ExchangeFailed | PlaudSignInOutcome::SignedIn => {
                    "couldn't finish signing in"
                }
                PlaudSignInOutcome::Cancelled => "sign-in cancelled",
                PlaudSignInOutcome::Expired => "the sign-in attempt timed out",
                PlaudSignInOutcome::Busy => "another sign-in or sync is already running",
                PlaudSignInOutcome::Storage => "couldn't save the sign-in in this journal",
            };
            failure(
                "",
                &format!(
                    "{}\n",
                    solstone_core_cli_copy::strings(
                        "journal.cli.importer.plaud.connect.failed",
                        [("detail", detail)],
                    )
                ),
                1,
            )
        }
    }
}

fn run_plaud_sync(journal_path: &Path, options: &Options) -> CliRun {
    use std::sync::Arc;

    let _sync_lock = match solstone_core_import::plaud::hold_sync_lock(journal_path) {
        Ok(lock) => lock,
        Err(_) => {
            return failure(
                "",
                &format!(
                    "{}\n",
                    solstone_core_cli_copy::text("journal.cli.importer.plaud.sync.busy")
                ),
                1,
            );
        }
    };
    let http: Arc<dyn solstone_core_oauth::ssrf::OAuthHttpClient> =
        Arc::new(solstone_core_oauth::ssrf::SsrfHttpsClient::new());
    let manager = Arc::new(solstone_core_oauth::store::OAuthCredentialManager::new(
        journal_path.to_path_buf(),
        solstone_core_import::plaud::service_id(),
        Arc::clone(&http),
    ));
    let mut catalogue = solstone_core_import::plaud::PlaudMcpClient::with_manager(
        Arc::clone(&manager),
        Arc::clone(&http),
        solstone_core_import::plaud::PLAUD_MCP_URL.to_owned(),
    );
    let mut download = solstone_core_import::plaud::PlaudMcpClient::with_manager(
        manager,
        http,
        solstone_core_import::plaud::PLAUD_MCP_URL.to_owned(),
    );
    let mut pipeline = GenericAudioPipeline { journal_path };
    run_plaud_with(
        journal_path,
        options.save,
        &mut catalogue,
        &mut download,
        &mut pipeline,
    )
}

pub(crate) fn run_plaud_with(
    journal_path: &Path,
    save: bool,
    catalogue: &mut dyn PlaudCatalogue,
    download: &mut dyn PlaudDownload,
    pipeline: &mut dyn ImportPipeline,
) -> CliRun {
    let credential = OAuthPlaudCredential::read(journal_path);
    let manifests = ImportFilenameMatches { journal_path };
    let clock = SystemSyncClock;
    let mut state_writer = FilesystemPlaudStateWriter;
    let preview = PlaudPreviewSeams {
        credential: &credential,
        catalogue,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut state_writer,
    };
    if !save {
        let request = PlaudSyncRequest::<SyncPreviewRequest>::new(journal_path.to_path_buf());
        let mut seams = preview;
        return match sync_plaud_preview(&request, &mut seams) {
            Ok(outcome) => success(cli_render::plaud_sync_preview(state_file_count(
                &outcome.state,
            ))),
            Err(error) => {
                if let PlaudSyncError::Operation(kind) = &error {
                    solstone_core_import::plaud::record_sync_attention(
                        journal_path,
                        Some(solstone_core_import::plaud::attention_code_for_failure(
                            *kind,
                        )),
                    );
                }
                failure("", &format!("{error}\n"), 1)
            }
        };
    }
    let request = PlaudSyncRequest::<SyncSaveRequest>::new(journal_path.to_path_buf());
    let mut seams = PlaudSaveSeams {
        preview,
        download,
        pipeline,
    };
    match sync_plaud_save(&request, &mut seams) {
        Ok(outcome) => sync_saved(
            "Plaud",
            outcome.downloaded,
            &outcome.errors,
            outcome.stopped,
        ),
        Err(error) => {
            if let PlaudSyncError::Operation(kind) = &error {
                solstone_core_import::plaud::record_sync_attention(
                    journal_path,
                    Some(solstone_core_import::plaud::attention_code_for_failure(
                        *kind,
                    )),
                );
            }
            failure("", &format!("{error}\n"), 1)
        }
    }
}

/// A sync that saved: a count on stdout, and each item that failed on stderr, failing the run.
///
/// A stop that skipped a later unit keeps that count and fails the run. The
/// skipped unit is not an item error.
fn sync_saved(backend: &str, saved: u64, errors: &[String], stopped: bool) -> CliRun {
    let stdout = cli_render::sync_save_complete(backend, saved, errors.len());
    if stopped {
        let mut stderr = String::from("import stopped\n");
        for error in errors {
            stderr.push_str(error);
            stderr.push('\n');
        }
        return failure(&stdout, &stderr, 1);
    }
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
        // A synced recording has no owner to choose its time. When the resolver finds no
        // creation evidence, it lands at the file's modified time in the owner's zone.
        let fallback_timestamp = if request.timestamp.is_none() {
            file_mtime_stamp(
                request.source,
                &solstone_core_journal_config::owner_zone(self.journal_path),
            )
        } else {
            None
        };
        let options = Options {
            media: Some(media.clone()),
            timestamp: request.timestamp.map(str::to_owned),
            fallback_timestamp,
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
                let timestamp = if let Some(instant) = request.source_instant {
                    solstone_core_import::source_timestamp_for_preview(
                        self.journal_path,
                        &timestamp,
                        request.source,
                    )
                    .map_err(|error| error.to_string())?
                    .with_source_instant(instant)
                } else {
                    timestamp
                };
                let (run, _) = run_audio(&media, &options, self.journal_path, &timestamp, false);
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

/// A file's last-modified time, as a wall-clock stamp in the owner zone.
fn file_mtime_stamp(path: &Path, zone: &TimeZone) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let stamp = Timestamp::try_from(modified)
        .ok()?
        .to_zoned_datetime(zone.clone())
        .strftime("%Y%m%d_%H%M%S")
        .to_string();
    solstone_core_import::validate_timestamp(&stamp)
        .ok()
        .map(|_| stamp)
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
        solstone_core_time::zoned_rfc3339(
            &jiff::ZonedDateTime::now(),
            solstone_core_time::Precision::AutoSi,
            false,
        )
    }
}

/// Indicates whether the journal has an active Plaud OAuth sign-in (`config/plaud-sign-in.json`).
/// The MCP client loads and refreshes the real credential directly through `OAuthCredentialManager`,
/// so raw tokens never flow through the generic sync seam.
struct OAuthPlaudCredential {
    connected: bool,
}

impl OAuthPlaudCredential {
    fn read(journal_path: &Path) -> Self {
        Self {
            connected: solstone_core_import::plaud::is_connected(journal_path),
        }
    }
}

impl PlaudCredential for OAuthPlaudCredential {
    fn access_token(&self) -> Option<&str> {
        self.connected.then_some("oauth")
    }
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
        let mut remote_ids = std::collections::HashMap::new();
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
            let Ok(metadata) =
                solstone_core_import::read_import_metadata(self.journal_path, &import_id)
            else {
                continue;
            };
            if let Some(remote_id) = metadata.get("plaud_file_id").and_then(Value::as_str) {
                if entry.path().join("manifest.json").is_file()
                    && solstone_core_import::project_import_result(self.journal_path, &import_id)
                        .status
                        == solstone_core_import::ProjectionStatus::Success
                {
                    remote_ids.insert(remote_id.to_owned(), import_id);
                }
                continue;
            }
            // A filename remains a legacy hint for hand imports only. New records
            // never identify somebody else's remote recording by its title.
            if solstone_core_import::originals::is_new_import(&metadata) {
                continue;
            }
            let Some(original) = metadata
                .get("original_filename")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
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
                if let Some(import_id) = remote_ids.get(&file.id) {
                    return Some((file.id.clone(), import_id.clone()));
                }
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

#[derive(Clone, Default)]
struct Options {
    media: Option<String>,
    timestamp: Option<String>,
    source: Option<String>,
    sync: Option<String>,
    connect: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    facet: Option<String>,
    no_facet: bool,
    listed_moves_shown: Option<usize>,
    /// The import a page start runs under; only an Apple Health card sends it.
    import_id: Option<String>,
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
    snapshot_digest: Option<String>,
    preview_token: Option<String>,
    preview_zone: Option<String>,
    preview_stamp: Option<String>,
    fallback_timestamp: Option<String>,
    audio_archive: Option<PathBuf>,
    archive_identity: Option<String>,
    source_zone: Option<String>,
    resume: Option<String>,
    audio_archive_status: Option<String>,
    audio_archive_members: Option<String>,
    offset: Option<u64>,
    limit: Option<u32>,
}

enum ParsedCommand {
    Help,
    ListImporters {
        json: bool,
    },
    Backends,
    Connect {
        backend: String,
    },
    Sync {
        backend: String,
        options: Options,
    },
    /// A form that used to bring a folder of notes in. It only refuses.
    RetiredNotesFolder,
    AudioArchive(crate::audio_archive::AudioArchiveDispatchOptions),
    AudioArchiveStatus {
        run_id: String,
        json: bool,
    },
    AudioArchiveMembers {
        run_id: String,
        offset: u64,
        limit: Option<u32>,
        json: bool,
    },
    Import(Options),
}

/// The argument value of the retired notes-folder import and sync.
const RETIRED_NOTES_SOURCE: &str = "obsidian";

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
    if options.sync.as_deref() == Some(RETIRED_NOTES_SOURCE)
        || options.source.as_deref() == Some(RETIRED_NOTES_SOURCE)
    {
        return Ok(ParsedCommand::RetiredNotesFolder);
    }
    if options.archive_identity.is_some() && options.audio_archive.is_none() {
        return Err("--archive-identity is only valid with --audio-archive".to_owned());
    }
    if (options.facet.is_some() || options.no_facet)
        && (options.audio_archive.is_some()
            || options.sync.as_deref().is_some_and(|sync| sync != "oura"))
    {
        return Err(
            "--facet is only valid when importing one audio or text file or a body import"
                .to_owned(),
        );
    }
    if let Some(audio_archive) = options.audio_archive {
        if !positionals.is_empty() {
            return Err(format!("unrecognized arguments: {}", positionals.join(" ")));
        }
        if options.offset.is_some() || options.limit.is_some() {
            return Err("unrecognized arguments: --offset/--limit are only valid with --audio-archive-members".to_owned());
        }
        if options.audio_archive_status.is_some() || options.audio_archive_members.is_some() {
            return Err("unrecognized arguments: conflicting audio archive commands".to_owned());
        }
        return Ok(ParsedCommand::AudioArchive(
            crate::audio_archive::AudioArchiveDispatchOptions {
                path: audio_archive,
                source_zone: options.source_zone,
                resume: options.resume,
                expected_identity: options.archive_identity,
                expected_owner_zone: options.preview_zone,
                dry_run: options.dry_run,
                json: options.json,
            },
        ));
    }
    if let Some(run_id) = options.audio_archive_status {
        if !positionals.is_empty() {
            return Err(format!("unrecognized arguments: {}", positionals.join(" ")));
        }
        if options.offset.is_some() || options.limit.is_some() {
            return Err("unrecognized arguments: --offset/--limit are only valid with --audio-archive-members".to_owned());
        }
        if options.resume.is_some()
            || options.source_zone.is_some()
            || options.audio_archive_members.is_some()
        {
            return Err("unrecognized arguments: conflicting audio archive flags".to_owned());
        }
        return Ok(ParsedCommand::AudioArchiveStatus {
            run_id,
            json: options.json,
        });
    }
    if let Some(run_id) = options.audio_archive_members {
        if !positionals.is_empty() {
            return Err(format!("unrecognized arguments: {}", positionals.join(" ")));
        }
        if options.resume.is_some() || options.source_zone.is_some() {
            return Err("unrecognized arguments: conflicting audio archive flags".to_owned());
        }
        return Ok(ParsedCommand::AudioArchiveMembers {
            run_id,
            offset: options.offset.unwrap_or(0),
            limit: options.limit,
            json: options.json,
        });
    }

    if options.offset.is_some() || options.limit.is_some() {
        return Err(
            "unrecognized arguments: --offset/--limit are only valid with --audio-archive-members"
                .to_owned(),
        );
    }
    if options.resume.is_some() {
        return Err(
            "unrecognized arguments: --resume is only valid with --audio-archive".to_owned(),
        );
    }
    if options.source_zone.is_some() {
        return Err(
            "unrecognized arguments: --source-zone is only valid with --audio-archive".to_owned(),
        );
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
        "--preview-token"
            | "--preview-zone"
            | "--preview-stamp"
            | "--fallback-timestamp"
            | "--snapshot-digest"
            | "--timestamp"
            | "--facet"
            | "--listed-moves-shown"
            | "--import-id"
            | "--setting"
            | "--source"
            | "--sync"
            | "--path"
            | "--window-days"
            | "--connect"
            | "--date-from"
            | "--date-to"
            | "--audio-archive"
            | "--archive-identity"
            | "--source-zone"
            | "--resume"
            | "--audio-archive-status"
            | "--audio-archive-members"
            | "--offset"
            | "--limit"
    )
}

fn assign_value(options: &mut Options, name: &str, value: &str) -> Result<(), String> {
    match name {
        "--preview-zone" => options.preview_zone = Some(value.to_owned()),
        "--preview-token" => options.preview_token = Some(value.to_owned()),
        "--preview-stamp" => options.preview_stamp = Some(value.to_owned()),
        "--fallback-timestamp" => options.fallback_timestamp = Some(value.to_owned()),
        "--snapshot-digest" => options.snapshot_digest = Some(value.to_owned()),
        "--timestamp" => options.timestamp = Some(value.to_owned()),
        "--source" => options.source = Some(value.to_owned()),
        "--sync" => options.sync = Some(value.to_owned()),
        "--connect" => options.connect = Some(value.to_owned()),
        "--date-from" => options.date_from = Some(value.to_owned()),
        "--date-to" => options.date_to = Some(value.to_owned()),
        "--path" => {
            let _ = value;
        }
        "--window-days" => {
            options.window_days = Some(
                value
                    .parse()
                    .map_err(|_| format!("argument --window-days: invalid int value: '{value}'"))?,
            )
        }
        "--facet" => options.facet = Some(value.to_owned()),
        "--listed-moves-shown" => {
            options.listed_moves_shown = Some(value.parse().map_err(|_| {
                format!("argument --listed-moves-shown: invalid int value: '{value}'")
            })?);
        }
        "--setting" => options.setting = Some(value.to_owned()),
        "--import-id" => options.import_id = Some(value.to_owned()),
        "--audio-archive" => options.audio_archive = Some(PathBuf::from(value)),
        "--archive-identity" => {
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("--archive-identity must be a SHA-256 identity".to_owned());
            }
            options.archive_identity = Some(value.to_ascii_lowercase());
        }
        "--source-zone" => options.source_zone = Some(value.to_owned()),
        "--resume" => options.resume = Some(value.to_owned()),
        "--audio-archive-status" => options.audio_archive_status = Some(value.to_owned()),
        "--audio-archive-members" => options.audio_archive_members = Some(value.to_owned()),
        "--offset" => {
            options.offset = Some(
                value
                    .parse()
                    .map_err(|_| format!("argument --offset: invalid int value: '{value}'"))?,
            )
        }
        "--limit" => {
            options.limit = Some(
                value
                    .parse()
                    .map_err(|_| format!("argument --limit: invalid int value: '{value}'"))?,
            )
        }
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
        "--no-facet" => options.no_facet = true,
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
            // A stalled segment is still processing, not failed; saying "failed" for it sent
            // a reader looking for a failure that never happened. The exit stays non-zero for
            // both, because neither finished while the importer waited.
            let mut lines = String::new();
            if !processing.failed_segments.is_empty() {
                lines.push_str(&format!(
                    "audio import processing failed: {}\n",
                    processing.failed_segments.join(", ")
                ));
            }
            if !processing.stalled_segments.is_empty() {
                lines.push_str(&format!(
                    "audio import still processing when the importer stopped waiting: {}\n",
                    processing.stalled_segments.join(", ")
                ));
            }
            if !lines.is_empty() {
                return failure("", &lines, 1);
            }
            // A chunk left out keeps the import a success with gaps, but the owner is told
            // which part is missing before trying the file again.
            let left_out = outcome
                .dropped_chunks()
                .iter()
                .map(|chunk| {
                    format!(
                        "{} to {}",
                        recording_offset(chunk.start_offset_seconds),
                        recording_offset(chunk.start_offset_seconds + chunk.duration_seconds)
                    )
                })
                .collect::<Vec<_>>();
            let mut run = success(format!("Generic audio import complete: {outcome:?}\n"));
            if !left_out.is_empty() {
                run.stderr = format!(
                    "part of this recording couldn't be imported: {} into the recording. the other parts are in your journal, and importing the file again would add them a second time.\n",
                    left_out.join(", ")
                );
            }
            run
        }
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

/// A position in a recording as `m:ss`, or `h:mm:ss` past the first hour.
fn recording_offset(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn argparse_error(arguments: String) -> CliRun {
    failure(
        "",
        &format!(
            "usage: solstone journal importer [-h] [options] media [timestamp]\nsolstone journal importer: error: {arguments}\n"
        ),
        2,
    )
}
pub(crate) fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}
pub(crate) fn failure(stdout: &str, stderr: &str, exit_code: i32) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
    }
}

#[cfg(all(test, unix, feature = "full-tests"))]
#[path = "sync_effect_tests.rs"]
mod sync_effect_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_off_apple_export_is_refused_in_plain_words() {
        let scratch = tempfile::tempdir().unwrap();
        std::fs::write(scratch.path().join("export.xml"), "<HealthData><Record").unwrap();
        let options = Options {
            confirm_body_save: true,
            ..Options::default()
        };
        let run = crate::body_commands::save_apple(
            scratch.path().to_str().unwrap(),
            &body_command(&options),
            &scratch.path().join("journal"),
        );
        assert_eq!(run.exit_code, 1, "{}", run.stderr);
        assert_eq!(
            run.stderr.trim_end(),
            solstone_core_import_sources::apple_export::EXPORT_INCOMPLETE_TEXT
        );
    }

    #[test]
    fn force_notice_uses_positional_date_and_actual_binding_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("note.txt");
        fs::write(&source, "same note").unwrap();
        let old = solstone_core_import::validate_timestamp("20260101_120000").unwrap();
        solstone_core_import::bind_import_record(root.path(), &old, Some(&source)).unwrap();
        let args = vec![
            source.display().to_string(),
            "20260201_120000".into(),
            "--force".into(),
            "--json".into(),
        ];
        let before = solstone_core_import::snapshot::fingerprint(root.path()).unwrap();
        let notice = force_day_notice_with(&args, root.path(), || Ok(()))
            .unwrap()
            .unwrap();
        assert!(notice.contains("previous import used 20260101"));
        assert!(notice.contains("will use 20260201"));
        assert_eq!(
            before,
            solstone_core_import::snapshot::fingerprint(root.path()).unwrap()
        );
        let refused = force_day_notice_with(&args, root.path(), || {
            Err(SupervisorRefusal::SpawnedUnavailable)
        })
        .unwrap_err();
        assert_eq!(refused.exit_code, 75);
        assert!(refused.stdout.is_empty() && refused.stderr.is_empty());
    }

    #[test]
    fn local_preview_binds_bytes_and_owner_zone_before_mutation() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        let config = root.path().join("config/journal.json");
        fs::write(&config, r#"{"identity":{"timezone":"UTC"}}"#).unwrap();
        let source = root.path().join("notes.txt");
        fs::write(&source, "a note").unwrap();
        let args = vec![source.display().to_string(), "20261011_120000".into()];
        let preview = preview_arguments_with(&args, root.path(), || Ok(()))
            .unwrap()
            .unwrap();
        let Ok(ParsedCommand::Import(options)) = parse_arguments(&preview) else {
            panic!("import");
        };
        assert_eq!(options.preview_zone.as_deref(), Some("UTC"));
        assert_eq!(
            options.snapshot_digest.as_deref(),
            Some(
                solstone_core_import::snapshot::fingerprint(&source)
                    .unwrap()
                    .as_str()
            )
        );
        fs::write(&config, r#"{"identity":{"timezone":"Asia/Tokyo"}}"#).unwrap();
        let CliOutcome::Rendered(run) = run_import(options, root.path(), Dispatch::Cli) else {
            panic!("refusal");
        };
        assert_eq!(run.exit_code, 1);
        assert!(run.stderr.contains("time zone changed"));
        assert!(!root.path().join("chronicle").exists());
    }

    #[test]
    fn preview_answers_the_supervisor_preflight_before_reading_the_source() {
        let root = tempfile::tempdir().unwrap();
        let args = vec!["missing-file".to_owned()];
        let spawned = preview_arguments_with(&args, root.path(), || {
            Err(SupervisorRefusal::SpawnedUnavailable)
        })
        .unwrap_err();
        assert_eq!(spawned, failure("", "", 75));
        let interactive =
            preview_arguments_with(&args, root.path(), || Err(SupervisorRefusal::Unavailable))
                .unwrap_err();
        assert_eq!(
            interactive,
            failure("", &format!("{SUPERVISOR_MESSAGE}\n"), 1)
        );
        let unreadable = preview_arguments_with(&args, root.path(), || Ok(())).unwrap_err();
        assert_eq!(unreadable.exit_code, 1);
        assert!(unreadable.stderr.starts_with("could not preview import: "));
        let dry_run = vec!["--dry-run".to_owned(), "missing-file".to_owned()];
        assert_eq!(
            preview_arguments_with(&dry_run, root.path(), || {
                Err(SupervisorRefusal::SpawnedUnavailable)
            }),
            Ok(None)
        );
    }

    fn recording(id: &str, filename: &str, fullname: &str) -> PlaudFile {
        PlaudFile {
            id: id.to_owned(),
            filename: filename.to_owned(),
            fullname: fullname.to_owned(),
            filesize: 1024,
            start_time: 1_773_230_400_000.0,
            duration: 95_000.0,
            is_trash: false,
        }
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

#[cfg(all(test, feature = "full-tests"))]
mod opus_audio_tests {
    use super::*;

    fn write_utc_zone(journal: &Path) {
        let config = journal.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
    }

    fn opus_fixture(folder: &Path, name: &str) -> PathBuf {
        let source = folder.join(name);
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/long_audio_remux_corpus/long-remux.opus"),
            &source,
        )
        .unwrap();
        source
    }

    fn step_back_inside_second_chunk(path: &Path) {
        fn crc(bytes: &[u8]) -> u32 {
            bytes.iter().fold(0_u32, |crc, byte| {
                let mut value = crc ^ (u32::from(*byte) << 24);
                for _ in 0..8 {
                    value = if value & 0x8000_0000 != 0 {
                        (value << 1) ^ 0x04C1_1DB7
                    } else {
                        value << 1
                    };
                }
                value
            })
        }
        let mut data = fs::read(path).unwrap();
        let mut position = 0;
        let mut changed = 0;
        while position < data.len() {
            assert_eq!(&data[position..position + 4], b"OggS");
            let segments = usize::from(data[position + 26]);
            let body: usize = data[position + 27..position + 27 + segments]
                .iter()
                .map(|lacing| usize::from(*lacing))
                .sum();
            let length = 27 + segments + body;
            let granule = i64::from_le_bytes(data[position + 6..position + 14].try_into().unwrap());
            if (420 * 48_000..440 * 48_000).contains(&granule) {
                let moved = granule - 60 * 48_000;
                data[position + 6..position + 14].copy_from_slice(&moved.to_le_bytes());
                data[position + 22..position + 26].fill(0);
                let checksum = crc(&data[position..position + length]);
                data[position + 22..position + 26].copy_from_slice(&checksum.to_le_bytes());
                changed += 1;
            }
            position += length;
        }
        assert!(changed > 0, "the fixture has pages in the second chunk");
        fs::write(path, data).unwrap();
    }

    fn opening_packet(path: &Path) -> (i64, Vec<u8>) {
        ffmpeg_next::init().unwrap();
        let mut input = ffmpeg_next::format::input(path).unwrap();
        let audio = input
            .streams()
            .best(ffmpeg_next::media::Type::Audio)
            .unwrap()
            .index();
        input
            .packets()
            .find(|(stream, _)| stream.index() == audio)
            .map(|(_, packet)| {
                (
                    packet.pts().unwrap(),
                    packet.data().unwrap_or_default().to_vec(),
                )
            })
            .unwrap()
    }

    #[test]
    fn an_import_with_a_part_left_out_is_not_marked_imported() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        let source = opus_fixture(folder.path(), "20250717_120000.opus");
        step_back_inside_second_chunk(&source);

        let timestamp = solstone_core_import::validate_timestamp("20250717_120000").unwrap();
        let (run, _) = run_audio(
            source.to_str().unwrap(),
            &Options::default(),
            journal.path(),
            &timestamp,
            false,
        );
        assert_eq!(
            run.exit_code, 0,
            "stdout={} stderr={}",
            run.stdout, run.stderr
        );
        let day = journal.path().join("chronicle/20250717/import.audio");
        assert!(day.join("120000_300/imported_audio.opus").is_file());
        assert!(
            !day.join("120500_300").exists(),
            "the second chunk was left out"
        );

        let hash = solstone_core_import::hash_source(&source).unwrap();
        let scan = solstone_core_import::find_manifest_by_hash(journal.path(), &hash).unwrap();
        assert!(
            scan.found.is_none(),
            "a partial import must stay importable: {:?}",
            scan.found.map(|found| found.path)
        );
    }

    #[test]
    fn an_opus_import_keeps_its_opening_packet() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let folder = tempfile::tempdir().unwrap();
        let source = opus_fixture(folder.path(), "20250717_130000.opus");
        let (source_pts, source_bytes) = opening_packet(&source);
        assert!(source_pts < 0, "the fixture opens before zero (pre-skip)");

        let timestamp = solstone_core_import::validate_timestamp("20250717_130000").unwrap();
        let (run, _) = run_audio(
            source.to_str().unwrap(),
            &Options::default(),
            journal.path(),
            &timestamp,
            false,
        );
        assert_eq!(
            run.exit_code, 0,
            "stdout={} stderr={}",
            run.stdout, run.stderr
        );
        let chunk = journal
            .path()
            .join("chronicle/20250717/import.audio/130000_300/imported_audio.opus");
        assert_eq!(opening_packet(&chunk), (source_pts, source_bytes));
    }
}
