// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};

use solstone_core_observer::{ObserverCommand, parse_observer_args};

macro_rules! speaker_resolve_usage {
    () => {
        "  solstone-core speaker-resolve <accumulate-voiceprints|write-owner-centroid|rebuild-owner-centroid|write-owner-candidate|read-owner-candidate|screen-owner-contamination|clear-owner-candidate|write-voiceprint|remove-voiceprint|backfill-voiceprint-last-seen|write-stub-labels|write-full-labels|patch-labels|restore-label-rows|append-correction|wipe-speaker-artifacts|identify|undo-identify|bootstrap-voiceprints|seed-from-imports|merge-names|backfill|backfill-status>\n"
    };
}

pub const USAGE: &str = concat!(
    "Usage:\n  solstone-core --version\n  solstone-core warm [--json]\n  solstone-core check [--json]\n  solstone-core assets\n  solstone-core doctor [--verbose] [--json | --jsonl] [--port PORT] [--feature NAME] [--readiness]\n  solstone-core journal-path [--journal PATH] [--create]\n  solstone-core indexer [--journal PATH] [--reset] [--rebuild-edges] [--rescan | --rescan-full | --rescan-file PATH]\n  solstone-core indexer search [QUERY] [--journal PATH] [--json] [--limit N] [--offset N] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax] [--counts] [--order relevance|recency]\n  solstone-core indexer counts [QUERY] [--journal PATH] [--json] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax]\n  solstone-core indexer agents [--journal PATH] [--json]\n  solstone-core indexer coverage [--journal PATH] [--json]\n  solstone-core journal-config read [--journal PATH]\n  solstone-core journal-config commit [--journal PATH] [--lock-timeout-ms N] --expect <fingerprint|absent>\n  solstone-core speaker-transcript-write\n  solstone-core observer [--json] <list|status|rename|revoke|reconcile|prune|create> ...\n",
    speaker_resolve_usage!(),
    "  solstone-core local probe-nvidia\n  solstone-core local plan\n  solstone-core local connect\n  solstone-core local install <pins|paths|fingerprint|verify|cuda|manifest|inspect|probe-binary|run> ...\n  solstone-core local generate\n  solstone-core generate --contract\n  solstone-core generate --one-shot\n  solstone-core generate --session --max-in-flight N\n  solstone-core cogitate --contract\n  solstone-core cogitate --talent-contract\n  solstone-core cogitate --one-shot\n  solstone-core brain refresh --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256 | --expect-absent] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain prerequisite-renewal --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain record-runtime-failure [--journal PATH]\n  solstone-core brain inspect [--journal PATH] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain fingerprint\n  solstone-core body rebuild [--journal PATH] [--json]\n  solstone-core body apple --source PATH [--detect | [--journal PATH] [--date-from DAY] [--date-to DAY] [--force] [--save [--confirm-body-save]] [--json]\n  solstone-core body oura connect [--journal PATH] [--json]\n  solstone-core body oura sync [--journal PATH] [--window-days N] [--save [--confirm-body-save | --scheduled]] [--json]\n  solstone-core transfer export --day YYYYMMDD --output PATH [--journal PATH]\n  solstone-core transfer import --archive PATH [--dry-run] [--journal PATH]\n  solstone-core transfer send --to LABEL [--day YYYYMMDD|YYYYMMDD-YYYYMMDD] [--dry-run] [--journal PATH]\n  solstone-core convey --port PORT [--journal PATH]\n  solstone-core grab [DAY [STREAM [SEGMENT [SCREEN [FRAME_ID[,FRAME_ID...]]]]]] [--out PATH] [--force] [--json] [-v | --verbose] [-d | --debug] [-h | --help]\n  solstone-core spl service [-v | --verbose] [-d | --debug]\n  solstone-core supervisor [PORT] [--no-daily] [--journal PATH] [--no-convey] [--no-cortex] [--no-spl] [--remote URL]\n",
    "  solstone-core navigate [-h | --help] [-f FACET | --facet FACET] [PATH]\n",
    "  solstone-core identity [-h | --help] <partner|health|briefing> ...\n",
    "  solstone-core settings [-h | --help] [-v | --verbose] [-d | --debug] [convey [status [--json]]]\n",
    "  solstone-core export --to LABEL [--only AREAS] [--day YYYYMMDD|YYYYMMDD-YYYYMMDD] [--dry-run] [--journal PATH]\n",
    "  solstone-core transcribe [-h] [--all] [--redo] [--backend {parakeet,parakeet-cpp,confidential}] [-v] [-d] [audio_path]\n",
    "  solstone-core facet-candidates [-h] [-v] [-d]\n  solstone-core install-models [--check | --force] [--variant {auto,cpu,cuda,coreml}]\n",
    "  solstone-core streams [args...]\n",
    "  solstone-core segment [args...]\n",
    "  solstone-core journal-stats [args...]\n",
    "  solstone-core reprocess [args...]\n",
    "  solstone-core backfill-processing-records [args...]\n"
);

pub const SPEAKER_RESOLVE_USAGE: &str = speaker_resolve_usage!();
/// The usage line the ERROR path prints, verbatim from the reference.
/// It names `journal grab`, not `solstone-core grab`: the owner-facing verb
/// is `journal grab`, and the native dispatch is a POSIX exec into the same
/// process, so naming the internal binary here names a command the owner
/// never typed.
pub const GRAB_USAGE: &str =
    "usage: journal grab [-h] [--out OUT] [--force] [--json] [-v] [-d] [args ...]\n";

/// The usage line native `journal navigate` prints for an argument error.
/// It names `journal navigate`, not `solstone-core navigate`, because that is
/// the command the owner typed.
pub const NAVIGATE_USAGE: &str = "usage: journal navigate [-h] [-f FACET | --facet FACET] [PATH]\n";

/// `journal navigate --help` in the owner-facing command vocabulary.
/// It names `journal navigate`, not `solstone-core navigate`, because that is
/// the command the owner typed.
pub const NAVIGATE_HELP: &str = concat!(
    "usage: journal navigate [-h] [-f FACET | --facet FACET] [PATH]\n",
    "\n",
    "Navigate the browser to a path and/or switch facet.\n",
    "\n",
    "positional arguments:\n",
    "  PATH                  URL path to navigate to.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  -f FACET, --facet FACET\n",
    "                        Facet to switch to.\n",
);

pub const IDENTITY_USAGE: &str = "usage: journal identity [-h] {partner,health,briefing} ...\n";

pub const IDENTITY_HELP: &str = concat!(
    "usage: journal identity [-h] {partner,health,briefing} ...\n",
    "\n",
    "Journal identity directory — partner.md and health.md.\n",
    "\n",
    "positional arguments:\n",
    "  {partner,health,briefing}\n",
    "    partner             Read or write identity/partner.md.\n",
    "    health              Read or regenerate the steward health surface.\n",
    "    briefing            Read the morning briefing.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
);

pub const IDENTITY_PARTNER_USAGE: &str =
    "usage: journal identity partner [-h] [-w] [--update-section HEADING] [--value VALUE]\n";

pub const IDENTITY_PARTNER_HELP: &str = concat!(
    "usage: journal identity partner [-h] [-w] [--update-section HEADING] [--value VALUE]\n",
    "\n",
    "Read or write identity/partner.md.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  -w, --write           Overwrite partner.md (content via --value or stdin).\n",
    "  --update-section HEADING\n",
    "                        Update a specific ## section of partner.md (content via\n",
    "                        --value or stdin).\n",
    "  --value VALUE         Content to write (alternative to stdin).\n",
);

pub const IDENTITY_HEALTH_USAGE: &str = "usage: journal identity health [-h] [--refresh]\n";

pub const IDENTITY_HEALTH_HELP: &str = concat!(
    "usage: journal identity health [-h] [--refresh]\n",
    "\n",
    "Read or regenerate the steward health surface.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --refresh             Regenerate identity/health.md through the steward talent.\n",
);

pub const IDENTITY_BRIEFING_USAGE: &str = "usage: journal identity briefing [-h] [-d DAY]\n";

pub const IDENTITY_BRIEFING_HELP: &str = concat!(
    "usage: journal identity briefing [-h] [-d DAY]\n",
    "\n",
    "Read the morning briefing.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  -d DAY, --day DAY     Specific day YYYYMMDD.\n",
);

pub const SETTINGS_USAGE: &str = "usage: journal settings [-h] [-v] [-d] {convey} ...\n";

pub const SETTINGS_HELP: &str = concat!(
    "usage: journal settings [-h] [-v] [-d] {convey} ...\n",
    "\n",
    "Manage local journal settings\n",
    "\n",
    "positional arguments:\n",
    "  {convey}\n",
    "    convey       Manage convey settings\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

pub const SETTINGS_CONVEY_USAGE: &str = "usage: journal settings convey [-h] {status} ...\n";

pub const SETTINGS_CONVEY_HELP: &str = concat!(
    "usage: journal settings convey [-h] {status} ...\n",
    "\n",
    "positional arguments:\n",
    "  {status}\n",
    "    status    Show convey bind and dashboard URL status\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
);

pub const SETTINGS_STATUS_USAGE: &str = "usage: journal settings convey status [-h] [--json]\n";

pub const SETTINGS_STATUS_HELP: &str = concat!(
    "usage: journal settings convey status [-h] [--json]\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --json      Print machine-readable status.\n",
);

/// `journal grab --help`, verbatim from the reference. The native verb
/// previously answered --help with the one-line usage above, losing every
/// argument description.
pub const GRAB_HELP: &str = concat!(
    "usage: journal grab [-h] [--out OUT] [--force] [--json] [-v] [-d] [args ...]\n",
    "\n",
    "Walk observed screen frames and optionally write frame images.\n",
    "\n",
    "positional arguments:\n",
    "  args           Path tokens: [day] [stream] [segment] [screen] [frame-\n",
    "                 id[,frame-id...]]\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  --out OUT      Write the selected frame image here (.png, .jpg, .jpeg, or\n",
    "                 .webp).\n",
    "  --force        Replace an existing output path.\n",
    "  --json         Emit JSON instead of table or plain output.\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

/// `journal observer`'s own usage block, captured verbatim from the Python
/// reference. The native verb must not print `solstone-core`'s top-level usage
/// when an owner mistypes an observer argument -- that names the wrong program.
pub const OBSERVER_USAGE: &str = concat!(
    "usage: journal observer [-h] [--json] [-v] [-d]\n",
    "                        {create,list,rename,revoke,status,reconcile,prune} ...\n",
);

/// `journal observer --help`, captured verbatim from the Python reference.
/// The cut left the native verb with no help at all: `--help` fell through the
/// observer parser (it is not one of its tokens) and became a usage error, so
/// an owner asking for help got exit 2 and three lines instead of exit 0 and
/// twenty-one.
pub const OBSERVER_HELP: &str = concat!(
    "usage: journal observer [-h] [--json] [-v] [-d]\n",
    "                        {create,list,rename,revoke,status,reconcile,prune} ...\n",
    "\n",
    "Manage observer registrations\n",
    "\n",
    "positional arguments:\n",
    "  {create,list,rename,revoke,status,reconcile,prune}\n",
    "    create              Explain retired manual observer creation\n",
    "    list                List all registered observers\n",
    "    rename              Rename an observer (affects future streams)\n",
    "    revoke              Revoke an observer registration\n",
    "    status              Show observer status details\n",
    "    reconcile           Collapse duplicate registrations per stream (oldest\n",
    "                        survives)\n",
    "    prune               Find or delete provable duplicate observer segments\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --json                Output as JSON\n",
    "  -v, --verbose         Enable verbose output\n",
    "  -d, --debug           Enable debug logging\n",
);

/// `journal observer prune --help`, captured verbatim from the reference.
/// Note it documents prune's own exit contract, which the native path honours:
/// 0 clean, 2 refusals present, 1 usage/error.
pub const OBSERVER_PRUNE_HELP: &str = concat!(
    "usage: journal observer prune [-h] (--day DAY | --day-range DAY_RANGE | --all)\n",
    "                              [--stream STREAM] [--execute] [--cross-start]\n",
    "\n",
    "Find byte-identical same-start observer duplicate segments. Canonical is the\n",
    "earliest same-start segment whose content is held by bytes or terminal proof.\n",
    "Opt-in cross-start mode also uses server-authored segment_original provenance\n",
    "after same-start pruning. Dry-run is the default and performs zero writes.\n",
    "Exit codes: 0 clean, 2 refusals present, 1 usage/error.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --day DAY             Prune one day (YYYYMMDD)\n",
    "  --day-range DAY_RANGE\n",
    "                        Prune inclusive range A..B\n",
    "  --all                 Scan every journal day\n",
    "  --stream STREAM       Limit to one stream\n",
    "  --execute             Delete provable duplicates; dry-run is the default.\n",
    "  --cross-start         Also prune different-start duplicates proven by\n",
    "                        server-authored segment_original provenance; runs\n",
    "                        after same-start. Off by default.\n",
);

/// The usage block argparse prints when `journal observer prune` itself fails
/// to parse. It is prune's own, not the observer-level one.
pub const OBSERVER_PRUNE_USAGE: &str = concat!(
    "usage: journal observer prune [-h] (--day DAY | --day-range DAY_RANGE | --all)\n",
    "                              [--stream STREAM] [--execute] [--cross-start]\n",
);

/// `journal transfer --help`, verbatim from the Python reference. The cut
/// left the native verb answering 64 with `solstone-core`'s top-level usage
/// for every invocation including --help, so the verb had no help at all.
pub const TRANSFER_HELP: &str = concat!(
    "usage: journal transfer [-h] [-v] [-d] {export,import,send} ...\n",
    "\n",
    "Transfer observed segments between solstone instances\n",
    "\n",
    "positional arguments:\n",
    "  {export,import,send}\n",
    "    export              Create archive from day's segments\n",
    "    import              Import archive into journal\n",
    "    send                Send segments to paired peer\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  -v, --verbose         Enable verbose output\n",
    "  -d, --debug           Enable debug logging\n",
);

/// The single usage line argparse prints on a `journal transfer` error. The
/// full help body belongs to `--help` only; argparse never prints it on an
/// error.
pub const TRANSFER_USAGE: &str =
    "usage: journal transfer [-h] [-v] [-d] {export,import,send} ...\n";

/// `journal export --help`, verbatim from the Python reference.
/// It advertises `--key`, which belongs to the RETIRED url-plus-key
/// destination mode. That is faithful, not a mistake: the reference still
/// lists the flag and refuses it at runtime, so this is the text an owner
/// sees today and matching it is the fidelity bar.
pub const EXPORT_HELP: &str = concat!(
    "usage: journal export [-h] --to TO [--key KEY] [--only ONLY] [--dry-run]\n",
    "                      [--day DAY] [-v] [-d]\n",
    "\n",
    "Export journal data to a remote solstone instance\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  --to TO        Remote URL (http:// or https://) or paired peer label\n",
    "  --key KEY      API key for URL mode\n",
    "  --only ONLY    Export only specific area (segments, entities, facets,\n",
    "                 imports, config)\n",
    "  --dry-run      Show what would be exported without sending\n",
    "  --day DAY      Day or range (YYYYMMDD or YYYYMMDD-YYYYMMDD)\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

/// The wrapped usage lines argparse prints on a `journal export` error.
/// argparse never prints the help body on an error.
pub const EXPORT_USAGE: &str = concat!(
    "usage: journal export [-h] --to TO [--key KEY] [--only ONLY] [--dry-run]\n",
    "                      [--day DAY] [-v] [-d]\n",
);

/// `journal transcribe --help`, verbatim from the Python reference.
pub const TRANSCRIBE_HELP: &str = concat!(
    "usage: journal transcribe [-h] [--all] [--redo]\n",
    "                          [--backend {parakeet,parakeet-cpp,confidential}]\n",
    "                          [-v] [-d]\n",
    "                          [audio_path]\n",
    "\n",
    "Transcribe audio files using pluggable STT and native speaker analysis\n",
    "\n",
    "positional arguments:\n",
    "  audio_path            Path to audio file in journal segment directory, e.g.\n",
    "                        HHMMSS_LEN/audio.flac\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --all                 Batch-transcribe all unprocessed audio segments in the\n",
    "                        journal\n",
    "  --redo                Reprocess file, overwriting existing outputs\n",
    "  --backend {parakeet,parakeet-cpp,confidential}\n",
    "                        STT backend to use (overrides config and resource-\n",
    "                        aware auto default)\n",
    "  -v, --verbose         Enable verbose output\n",
    "  -d, --debug           Enable debug logging\n",
);

/// The wrapped usage lines argparse prints on a `journal transcribe` error.
pub const TRANSCRIBE_USAGE: &str = concat!(
    "usage: journal transcribe [-h] [--all] [--redo]\n",
    "                          [--backend {parakeet,parakeet-cpp,confidential}]\n",
    "                          [-v] [-d]\n",
    "                          [audio_path]\n",
);

/// `journal facet-candidates --help`, verbatim from the reference.
pub const FACET_CANDIDATES_HELP: &str = concat!(
    "usage: journal facet-candidates [-h] [-v] [-d]\n",
    "\n",
    "Record recurring facet review candidates.\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

/// The wrapped usage line argparse prints on a `journal facet-candidates` error.
pub const FACET_CANDIDATES_USAGE: &str = "usage: journal facet-candidates [-h] [-v] [-d]\n";

/// `journal transfer export --help`, verbatim from the reference.
pub const TRANSFER_EXPORT_HELP: &str = concat!(
    "usage: journal transfer export [-h] --day DAY [--output OUTPUT]\n",
    "\n",
    "options:\n",
    "  -h, --help           show this help message and exit\n",
    "  --day DAY            Day to export (YYYYMMDD format)\n",
    "  --output, -o OUTPUT  Output archive path (default:\n",
    "                       scratch/{day}_{hostname}.tgz)\n",
);

/// `journal transfer import --help`, verbatim from the reference.
pub const TRANSFER_IMPORT_HELP: &str = concat!(
    "usage: journal transfer import [-h] --archive ARCHIVE [--dry-run]\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --archive, -a ARCHIVE\n",
    "                        Archive file to import\n",
    "  --dry-run             Validate archive without extracting\n",
);

/// `journal transfer send --help`, verbatim from the reference.
pub const TRANSFER_SEND_HELP: &str = concat!(
    "usage: journal transfer send [-h] --to TO [--day DAY] [--dry-run]\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --to TO     Paired peer label\n",
    "  --day DAY   Day or range (YYYYMMDD or YYYYMMDD-YYYYMMDD, default: all days)\n",
    "  --dry-run   Show what would be sent without uploading\n",
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Doctor(solstone_core_doctor::args::DoctorArgs),
    DoctorUsage(solstone_core_doctor::args::DoctorUsageError),
    Version,
    Assets,
    Warm {
        json: bool,
    },
    Check {
        json: bool,
    },
    JournalPath(JournalPathOptions),
    Indexer(Box<IndexerCommand>),
    JournalConfig(JournalConfigCommand),
    SpeakerTranscriptWrite,
    SpeakerResolve(SpeakerResolveCommand),
    Local(LocalCommand),
    Generate(GenerateCommand),
    Cogitate(CogitateCommand),
    Brain(BrainCommand),
    Body(BodyCommand),
    Transfer(TransferCommand),
    Export(ExportOptions),
    Transcribe(TranscribeOptions),
    Streams(Vec<OsString>),
    Segment(Vec<OsString>),
    Reprocess(Vec<OsString>),
    JournalStats(Vec<OsString>),
    Backfill(Vec<OsString>),
    FacetCandidates,
    InstallModels(InstallModelsOptions),
    Convey(ConveyOptions),
    Grab(GrabCommand),
    Spl(SplCommand),
    Supervisor(SupervisorOptions),
    Observer(ObserverCommand),
    Navigate {
        path: Option<String>,
        facet: Option<String>,
    },
    NavigateUsage,
    NavigateHelp,
    Identity(IdentityCommand),
    IdentityUsage,
    IdentityUnknownCommand(String),
    IdentityHelp,
    IdentityPartnerUsage,
    IdentityPartnerHelp,
    IdentityHealthUsage,
    IdentityHealthHelp,
    IdentityBriefingUsage,
    IdentityBriefingHelp,
    Settings(SettingsCommand),
    SettingsHelp,
    SettingsConveyHelp,
    SettingsStatusHelp,
    SettingsParseError(SettingsParseError),
    ObserverUsage,
    ObserverPruneUsage,
    ObserverHelp,
    ObserverPruneHelp,
    TransferUsage,
    ExportUsage,
    ExportHelp,
    TranscribeHelp,
    FacetCandidatesHelp,
    FacetCandidatesUsage,
    TransferHelp(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallModelsVariant {
    Auto,
    Cpu,
    Cuda,
    Coreml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallModelsOptions {
    pub check: bool,
    pub force: bool,
    pub variant: InstallModelsVariant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityCommand {
    Hydrate,
    Partner(IdentityPartnerOptions),
    Health(IdentityHealthOptions),
    Briefing(IdentityBriefingOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityPartnerOptions {
    pub write: bool,
    pub update_section: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityHealthOptions {
    pub refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityBriefingOptions {
    pub day: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCommand {
    RootFallbackHelp,
    Convey(SettingsConveyCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsConveyCommand {
    FallbackHelp,
    Status { json: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsParseError {
    InvalidSection(String),
    InvalidConveyCommand(String),
    UnrecognizedArgument(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferCommand {
    Export(TransferExportOptions),
    Import(TransferImportOptions),
    Send(TransferSendOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExportOptions {
    pub day: String,
    pub output: OsString,
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferImportOptions {
    pub archive: OsString,
    pub dry_run: bool,
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferSendOptions {
    pub to: String,
    pub day: Option<String>,
    pub dry_run: bool,
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub to: String,
    pub only: Option<String>,
    pub day: Option<String>,
    pub dry_run: bool,
    /// Parsed only so the binary can reject the retired option explicitly.
    pub key: Option<String>,
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribeOptions {
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyCommand {
    Rebuild(BodyRebuildOptions),
    Apple(BodyAppleOptions),
    Oura(BodyOuraCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyOuraCommand {
    Connect(BodyOuraConnectOptions),
    Sync(BodyOuraSyncOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyRebuildOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyAppleOptions {
    pub source: OsString,
    pub detect: bool,
    pub journal_override: Option<OsString>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub save: bool,
    pub confirm_body_save: bool,
    pub force: bool,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyOuraConnectOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyOuraSyncOptions {
    pub journal_override: Option<OsString>,
    pub window_days: Option<u64>,
    pub save: bool,
    pub confirm_body_save: bool,
    pub scheduled: bool,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConveyOptions {
    pub port: u16,
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorOptions {
    pub port: u16,
    pub journal_override: Option<OsString>,
    pub no_daily: bool,
    pub no_convey: bool,
    pub no_cortex: bool,
    pub no_spl: bool,
    pub remote: Option<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerResolveCommand {
    AccumulateVoiceprints,
    WriteOwnerCentroid,
    RebuildOwnerCentroid,
    WriteOwnerCandidate,
    ReadOwnerCandidate,
    ScreenOwnerContamination,
    ClearOwnerCandidate,
    WriteVoiceprint,
    RemoveVoiceprint,
    BackfillVoiceprintLastSeen,
    WriteStubLabels,
    WriteFullLabels,
    PatchLabels,
    RestoreLabelRows,
    AppendCorrection,
    WipeSpeakerArtifacts,
    Identify,
    UndoIdentify,
    BootstrapVoiceprints,
    SeedFromImports,
    MergeNames,
    Backfill,
    BackfillStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    ProbeNvidia,
    Plan,
    Connect,
    Install(InstallCommand),
    Generate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerateCommand {
    Contract,
    OneShot,
    Session(GenerateSessionOptions),
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CogitateCommand {
    Contract,
    TalentContract,
    OneShot,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateSessionOptions {
    pub arguments: Vec<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallCommand {
    PinsLocal,
    PathsLocal,
    FingerprintLocal,
    FingerprintMlx,
    VerifySha256,
    CudaTrust,
    ManifestVulkan,
    ManifestCuda,
    ManifestModel,
    InspectLocal,
    InspectMlx,
    InspectParakeet,
    ProbeBinary,
    RunLocal,
    RunMlx,
    PinsParakeet,
    PathsParakeet,
    FingerprintParakeet,
    RunParakeet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainCommand {
    RefreshSession(BrainRefreshSessionOptions),
    PrerequisiteRenewalSession(BrainPrerequisiteRenewalSessionOptions),
    RecordRuntimeFailure(BrainRuntimeFailureOptions),
    Inspect(BrainInspectOptions),
    Fingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainRefreshSessionOptions {
    pub journal_override: Option<OsString>,
    pub run_id: Option<String>,
    pub expect: Option<BrainRefreshExpectArg>,
    pub bundled_runtime_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainRefreshExpectArg {
    Absent,
    Sha256(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainPrerequisiteRenewalSessionOptions {
    pub journal_override: Option<OsString>,
    pub run_id: Option<String>,
    pub expected_fingerprint_sha256: Option<String>,
    pub bundled_runtime_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainRuntimeFailureOptions {
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainInspectOptions {
    pub journal_override: Option<OsString>,
    pub bundled_runtime_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalConfigCommand {
    Read(JournalConfigReadOptions),
    Commit(JournalConfigCommitOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalConfigReadOptions {
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalConfigCommitOptions {
    pub journal_override: Option<OsString>,
    pub lock_timeout_ms: Option<u64>,
    pub expect: JournalConfigExpectArg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalConfigExpectArg {
    Absent,
    Sha256(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplCommand {
    Service(ServiceOptions),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOptions {
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrabCommand {
    Help,
    Run(GrabOptions),
    ParseError(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabOptions {
    pub tokens: Vec<String>,
    pub out: Option<OsString>,
    pub force: bool,
    pub json: bool,
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPathOptions {
    pub journal_override: Option<OsString>,
    pub create: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
    pub reset: bool,
    pub rebuild_edges: bool,
    pub rescan: bool,
    pub rescan_full: bool,
    pub rescan_file: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexerCommand {
    Maintenance(IndexerOptions),
    Search(IndexerSearchOptions),
    Counts(IndexerCountsOptions),
    Agents(IndexerReadOptions),
    Coverage(IndexerReadOptions),
    PruneStream(IndexerPruneStreamOptions),
    PrunePaths(IndexerPrunePathsOptions),
    FoldEntityEdges(IndexerFoldEntityEdgesOptions),
    EdgeFingerprint(IndexerReadOptions),
    RebuildEdgesFingerprint(IndexerReadOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerPruneStreamOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
    pub stream: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerPrunePathsOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerFoldEntityEdgesOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
    pub source_id: String,
    pub target_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerQueryOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
    pub query: Option<String>,
    pub day: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub facet: Option<String>,
    pub agent: Option<String>,
    pub stream: Option<String>,
    pub time_bucket: Option<String>,
    pub relax: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerSearchOptions {
    pub query: IndexerQueryOptions,
    pub limit: usize,
    pub offset: usize,
    pub counts: bool,
    pub order: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerCountsOptions {
    pub query: IndexerQueryOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerReadOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageError;

pub fn evaluate_args(args: &[OsString]) -> Result<Command, UsageError> {
    match args {
        [command, rest @ ..] if command == OsStr::new("doctor") => {
            Ok(solstone_core_doctor::args::parse_doctor_args(rest)
                .map_or_else(Command::DoctorUsage, Command::Doctor))
        }
        [flag] if flag == OsStr::new("--version") => Ok(Command::Version),
        [command] if command == OsStr::new("assets") => Ok(Command::Assets),
        [command] if command == OsStr::new("warm") => Ok(Command::Warm { json: false }),
        [command, flag] if command == OsStr::new("warm") && flag == OsStr::new("--json") => {
            Ok(Command::Warm { json: true })
        }
        [command] if command == OsStr::new("check") => Ok(Command::Check { json: false }),
        [command, flag] if command == OsStr::new("check") && flag == OsStr::new("--json") => {
            Ok(Command::Check { json: true })
        }
        [command, rest @ ..] if command == OsStr::new("journal-path") => {
            parse_journal_path(rest).map(Command::JournalPath)
        }
        [command, rest @ ..] if command == OsStr::new("indexer") => {
            parse_indexer(rest).map(|command| Command::Indexer(Box::new(command)))
        }
        [command, rest @ ..] if command == OsStr::new("journal-config") => {
            parse_journal_config(rest).map(Command::JournalConfig)
        }
        [command] if command == OsStr::new("speaker-transcript-write") => {
            Ok(Command::SpeakerTranscriptWrite)
        }
        [command, rest @ ..] if command == OsStr::new("speaker-resolve") => {
            parse_speaker_resolve(rest).map(Command::SpeakerResolve)
        }
        [command, rest @ ..] if command == OsStr::new("local") => {
            parse_local(rest).map(Command::Local)
        }
        [command, rest @ ..] if command == OsStr::new("generate") => {
            Ok(Command::Generate(parse_generate(rest)))
        }
        [command, rest @ ..] if command == OsStr::new("cogitate") => {
            Ok(Command::Cogitate(parse_cogitate(rest)))
        }
        [command, rest @ ..] if command == OsStr::new("brain") => {
            parse_brain(rest).map(Command::Brain)
        }
        [command, rest @ ..] if command == OsStr::new("body") => {
            parse_body(rest).map(Command::Body)
        }
        [command, rest @ ..] if command == OsStr::new("transfer") => {
            // Help is not a token of the transfer parser either, so without this
            // interception `journal transfer --help` degrades into a usage error
            // that exits 64 and names solstone-core rather than the verb.
            let help = |a: &OsString| a == OsStr::new("--help") || a == OsStr::new("-h");
            if let [first, others @ ..] = rest
                && others.iter().any(help)
                && let Some(text) = match first.to_str() {
                    Some("export") => Some(TRANSFER_EXPORT_HELP),
                    Some("import") => Some(TRANSFER_IMPORT_HELP),
                    Some("send") => Some(TRANSFER_SEND_HELP),
                    _ => None,
                }
            {
                return Ok(Command::TransferHelp(text));
            }
            if rest.iter().any(help) {
                return Ok(Command::TransferHelp(TRANSFER_HELP));
            }
            // argparse exits 2 here, not 64.
            Ok(parse_transfer(rest).map_or(Command::TransferUsage, Command::Transfer))
        }
        [command, rest @ ..] if command == OsStr::new("export") => {
            // Help is not a token of the export parser; without this it degrades
            // into a usage error exiting 64 with solstone-core's usage -- the
            // defect three sibling verbs in this lane already shipped and had
            // to have repaired.
            let help = |a: &OsString| a == OsStr::new("--help") || a == OsStr::new("-h");
            if rest.iter().any(help) {
                return Ok(Command::ExportHelp);
            }
            // argparse exits 2 here, not 64.
            Ok(parse_export(rest).map_or(Command::ExportUsage, Command::Export))
        }
        [command, rest @ ..] if command == OsStr::new("navigate") => {
            let help = |a: &OsString| a == OsStr::new("--help") || a == OsStr::new("-h");
            let option_end = rest
                .iter()
                .position(|argument| argument == OsStr::new("--"))
                .unwrap_or(rest.len());
            if rest[..option_end].iter().any(help) {
                return Ok(Command::NavigateHelp);
            }
            Ok(parse_navigate(rest).unwrap_or(Command::NavigateUsage))
        }
        [command, rest @ ..] if command == OsStr::new("identity") => parse_identity(rest),
        [command, rest @ ..] if command == OsStr::new("settings") => parse_settings(rest),
        [command, rest @ ..] if command == OsStr::new("transcribe") => parse_transcribe(rest),
        [command, rest @ ..] if command == OsStr::new("streams") => {
            Ok(Command::Streams(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("segment") => {
            Ok(Command::Segment(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("reprocess") => {
            Ok(Command::Reprocess(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("journal-stats") => {
            Ok(Command::JournalStats(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("backfill-processing-records") => {
            Ok(Command::Backfill(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("facet-candidates") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                return Ok(Command::FacetCandidatesHelp);
            }
            Ok(parse_facet_candidates(rest)
                .map_or(Command::FacetCandidatesUsage, |_| Command::FacetCandidates))
        }
        [command, rest @ ..] if command == OsStr::new("install-models") => {
            parse_install_models(rest).map(Command::InstallModels)
        }
        [command, rest @ ..] if command == OsStr::new("convey") => {
            parse_convey(rest).map(Command::Convey)
        }
        [command, rest @ ..] if command == OsStr::new("grab") => {
            Ok(Command::Grab(parse_grab(rest)))
        }
        [command, rest @ ..] if command == OsStr::new("spl") => parse_spl(rest).map(Command::Spl),
        [command, rest @ ..] if command == OsStr::new("supervisor") => {
            parse_supervisor(rest).map(Command::Supervisor)
        }
        [command, rest @ ..] if command == OsStr::new("observer") => {
            // Help is not one of the observer parser's tokens, so it must be
            // intercepted here or it degrades into a usage error -- which is
            // exactly what the cut shipped.
            let help = |a: &OsString| a == OsStr::new("--help") || a == OsStr::new("-h");
            if let [first, others @ ..] = rest
                && first == OsStr::new("prune")
                && others.iter().any(help)
            {
                return Ok(Command::ObserverPruneHelp);
            }
            if rest.iter().any(help) {
                return Ok(Command::ObserverHelp);
            }
            // The reference exits 2 with `journal observer`'s usage, not 64 with
            // solstone-core's. Carry the failure as a command so main can render
            // it faithfully instead of collapsing it into UsageError.
            // A prune-level failure gets prune's usage block and prefix; an
            // observer-level one gets the observer's. argparse distinguishes
            // them and so must this.
            let prune = matches!(rest.first(), Some(first) if first == OsStr::new("prune"));
            Ok(parse_observer_args(rest).map_or(
                if prune {
                    Command::ObserverPruneUsage
                } else {
                    Command::ObserverUsage
                },
                Command::Observer,
            ))
        }
        _ => Err(UsageError),
    }
}

fn parse_settings(args: &[OsString]) -> Result<Command, UsageError> {
    let root_args_end = args
        .iter()
        .position(|argument| argument == OsStr::new("convey"))
        .unwrap_or(args.len());
    if args[..root_args_end].iter().any(is_help) {
        return Ok(Command::SettingsHelp);
    }

    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if matches!(
            argument.as_os_str(),
            value if value == OsStr::new("-v")
                || value == OsStr::new("--verbose")
                || value == OsStr::new("-d")
                || value == OsStr::new("--debug")
        ) {
            index += 1;
            continue;
        }
        if argument == OsStr::new("convey") {
            return parse_settings_convey(&args[index + 1..]);
        }
        let value = argument.to_string_lossy().into_owned();
        return if value.starts_with('-') {
            Ok(Command::SettingsParseError(
                SettingsParseError::UnrecognizedArgument(value),
            ))
        } else {
            Ok(Command::SettingsParseError(
                SettingsParseError::InvalidSection(value),
            ))
        };
    }
    Ok(Command::Settings(SettingsCommand::RootFallbackHelp))
}

fn parse_settings_convey(args: &[OsString]) -> Result<Command, UsageError> {
    let convey_args_end = args
        .iter()
        .position(|argument| argument == OsStr::new("status"))
        .unwrap_or(args.len());
    if args[..convey_args_end].iter().any(is_help) {
        return Ok(Command::SettingsConveyHelp);
    }

    let Some((argument, rest)) = args.split_first() else {
        return Ok(Command::Settings(SettingsCommand::Convey(
            SettingsConveyCommand::FallbackHelp,
        )));
    };
    if argument == OsStr::new("status") {
        return parse_settings_status(rest);
    }
    let value = argument.to_string_lossy().into_owned();
    if value.starts_with('-') {
        Ok(Command::SettingsParseError(
            SettingsParseError::UnrecognizedArgument(value),
        ))
    } else {
        Ok(Command::SettingsParseError(
            SettingsParseError::InvalidConveyCommand(value),
        ))
    }
}

fn parse_settings_status(args: &[OsString]) -> Result<Command, UsageError> {
    if args.iter().any(is_help) {
        return Ok(Command::SettingsStatusHelp);
    }
    let mut json = false;
    for argument in args {
        if argument == OsStr::new("--json") {
            json = true;
        } else {
            return Ok(Command::SettingsParseError(
                SettingsParseError::UnrecognizedArgument(argument.to_string_lossy().into_owned()),
            ));
        }
    }
    Ok(Command::Settings(SettingsCommand::Convey(
        SettingsConveyCommand::Status { json },
    )))
}

fn parse_identity(args: &[OsString]) -> Result<Command, UsageError> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Ok(Command::Identity(IdentityCommand::Hydrate));
    };
    if is_help(subcommand) {
        return Ok(Command::IdentityHelp);
    }
    match subcommand.to_str() {
        Some("partner") => {
            Ok(parse_identity_partner(rest).unwrap_or(Command::IdentityPartnerUsage))
        }
        Some("health") => Ok(parse_identity_health(rest).unwrap_or(Command::IdentityHealthUsage)),
        Some("briefing") => {
            Ok(parse_identity_briefing(rest).unwrap_or(Command::IdentityBriefingUsage))
        }
        Some(command) => Ok(Command::IdentityUnknownCommand(command.to_owned())),
        None => Ok(Command::IdentityUsage),
    }
}

fn parse_identity_partner(args: &[OsString]) -> Result<Command, UsageError> {
    if args.iter().any(is_help) {
        return Ok(Command::IdentityPartnerHelp);
    }
    let mut write = false;
    let mut update_section = None;
    let mut value = None;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if let Some(attached) = attached_value(argument, "--update-section=") {
            update_section = Some(attached.to_owned());
            index += 1;
            continue;
        }
        if let Some(attached) = attached_value(argument, "--value=") {
            value = Some(attached.to_owned());
            index += 1;
            continue;
        }
        if argument == OsStr::new("--write") || argument == OsStr::new("-w") {
            write = true;
            index += 1;
            continue;
        }
        if argument == OsStr::new("--update-section") {
            let (next, next_index) = identity_option_value(args, index)?;
            update_section = Some(next);
            index = next_index;
            continue;
        }
        if argument == OsStr::new("--value") {
            let (next, next_index) = identity_option_value(args, index)?;
            value = Some(next);
            index = next_index;
            continue;
        }
        return Ok(Command::IdentityPartnerUsage);
    }
    Ok(Command::Identity(IdentityCommand::Partner(
        IdentityPartnerOptions {
            write,
            update_section,
            value,
        },
    )))
}

fn parse_identity_health(args: &[OsString]) -> Result<Command, UsageError> {
    if args.iter().any(is_help) {
        return Ok(Command::IdentityHealthHelp);
    }
    let mut refresh = false;
    for argument in args {
        if argument == OsStr::new("--refresh") {
            refresh = true;
        } else {
            return Ok(Command::IdentityHealthUsage);
        }
    }
    Ok(Command::Identity(IdentityCommand::Health(
        IdentityHealthOptions { refresh },
    )))
}

fn parse_identity_briefing(args: &[OsString]) -> Result<Command, UsageError> {
    if args.iter().any(is_help) {
        return Ok(Command::IdentityBriefingHelp);
    }
    let mut day = None;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if let Some(attached) = attached_value(argument, "--day=") {
            day = Some(attached.to_owned());
            index += 1;
            continue;
        }
        if let Some(attached) = attached_short_value(argument, "-d") {
            day = Some(attached.to_owned());
            index += 1;
            continue;
        }
        if argument == OsStr::new("--day") || argument == OsStr::new("-d") {
            let (next, next_index) = identity_option_value(args, index)?;
            day = Some(next);
            index = next_index;
            continue;
        }
        return Ok(Command::IdentityBriefingUsage);
    }
    if day.as_deref().is_some_and(|value| !is_identity_day(value)) {
        return Ok(Command::IdentityBriefingUsage);
    }
    Ok(Command::Identity(IdentityCommand::Briefing(
        IdentityBriefingOptions { day },
    )))
}

fn is_help(argument: &OsString) -> bool {
    argument == OsStr::new("--help") || argument == OsStr::new("-h")
}

fn attached_value<'a>(argument: &'a OsString, prefix: &str) -> Option<&'a str> {
    argument.to_str()?.strip_prefix(prefix)
}

fn attached_short_value<'a>(argument: &'a OsString, option: &str) -> Option<&'a str> {
    argument
        .to_str()?
        .strip_prefix(option)
        .filter(|value| !value.is_empty())
}

fn identity_option_value(args: &[OsString], index: usize) -> Result<(String, usize), UsageError> {
    let value = args.get(index + 1).ok_or(UsageError)?;
    let value = value
        .to_str()
        .filter(|value| !value.starts_with('-'))
        .ok_or(UsageError)?;
    Ok((value.to_owned(), index + 2))
}

fn is_identity_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_navigate(args: &[OsString]) -> Result<Command, UsageError> {
    let mut path = None;
    let mut facet = None;
    let mut literal = false;
    let mut index = 0;

    while index < args.len() {
        let argument = &args[index];
        if !literal && argument == OsStr::new("--") {
            literal = true;
            index += 1;
            continue;
        }
        if !literal
            && let Some(value) = argument
                .to_str()
                .and_then(|argument| argument.strip_prefix("--facet="))
        {
            if facet.is_some() {
                return Err(UsageError);
            }
            facet = Some(value.to_owned());
            index += 1;
            continue;
        }
        if !literal
            && let Some(value) = argument
                .to_str()
                .and_then(|argument| argument.strip_prefix("-f"))
                .filter(|value| !value.is_empty())
        {
            if facet.is_some() {
                return Err(UsageError);
            }
            facet = Some(value.to_owned());
            index += 1;
            continue;
        }
        if !literal && (argument == OsStr::new("--facet") || argument == OsStr::new("-f")) {
            if facet.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            // Only separate option values reject dash-leading tokens; attached
            // forms (`--facet=-x` and `-f-x`) take their suffix literally.
            let value = value
                .to_str()
                .filter(|value| !value.starts_with('-'))
                .ok_or(UsageError)?;
            facet = Some(value.to_owned());
            index += 2;
            continue;
        }
        if !literal && argument.to_str().is_none_or(|value| value.starts_with('-')) {
            return Err(UsageError);
        }
        if path.is_some() {
            return Err(UsageError);
        }
        path = Some(argument.to_str().ok_or(UsageError)?.to_owned());
        index += 1;
    }

    Ok(Command::Navigate { path, facet })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum GrabParseState {
    LeadingOptions,
    Positionals,
    TrailingOptions,
}

fn parse_grab(args: &[OsString]) -> GrabCommand {
    let mut state = GrabParseState::LeadingOptions;
    let mut end_of_options = false;
    let mut tokens = Vec::new();
    let mut out = None;
    let mut force = false;
    let mut json = false;
    let mut verbose = false;
    let mut debug = false;
    let mut index = 0;

    while index < args.len() {
        let argument = args[index].as_os_str();
        if !end_of_options && argument == OsStr::new("--") {
            end_of_options = true;
            index += 1;
            continue;
        }
        if !end_of_options && is_grab_option(argument) {
            if argument == OsStr::new("-h") || argument == OsStr::new("--help") {
                return GrabCommand::Help;
            }
            if state == GrabParseState::Positionals {
                state = GrabParseState::TrailingOptions;
            }
            if argument == OsStr::new("--json") {
                json = true;
            } else if argument == OsStr::new("--force") {
                force = true;
            } else if argument == OsStr::new("-v") || argument == OsStr::new("--verbose") {
                verbose = true;
            } else if argument == OsStr::new("-d") || argument == OsStr::new("--debug") {
                debug = true;
            } else if argument == OsStr::new("--out") {
                let Some(value) = args.get(index + 1) else {
                    return grab_parse_error("argument --out: expected one argument");
                };
                if looks_like_grab_option(value.as_os_str()) {
                    return grab_parse_error("argument --out: expected one argument");
                }
                out = Some(value.clone());
                index += 1;
            } else if let Some(value) = argument
                .to_str()
                .and_then(|value| value.strip_prefix("--out="))
            {
                if value.is_empty() {
                    return grab_parse_error("argument --out: expected one argument");
                }
                out = Some(OsString::from(value));
            } else {
                return grab_parse_error(&format!(
                    "unrecognized arguments: {}",
                    argument.to_string_lossy()
                ));
            }
            index += 1;
            continue;
        }
        if !end_of_options && looks_like_grab_option(argument) {
            return grab_parse_error(&format!(
                "unrecognized arguments: {}",
                argument.to_string_lossy()
            ));
        }
        if state == GrabParseState::TrailingOptions {
            return grab_parse_error(&format!(
                "unrecognized arguments: {}",
                argument.to_string_lossy()
            ));
        }
        let Some(token) = argument.to_str() else {
            return grab_parse_error("path tokens must be valid Unicode");
        };
        tokens.push(token.to_owned());
        state = GrabParseState::Positionals;
        index += 1;
    }

    GrabCommand::Run(GrabOptions {
        tokens,
        out,
        force,
        json,
        verbose,
        debug,
    })
}

fn is_grab_option(argument: &OsStr) -> bool {
    matches!(
        argument.to_str(),
        Some(
            "-h" | "--help"
                | "--json"
                | "--force"
                | "-v"
                | "--verbose"
                | "-d"
                | "--debug"
                | "--out"
        )
    ) || argument
        .to_str()
        .is_some_and(|value| value.starts_with("--out="))
}

fn looks_like_grab_option(argument: &OsStr) -> bool {
    argument != OsStr::new("-")
        && argument
            .to_str()
            .is_some_and(|value| value.starts_with('-') && value.parse::<f64>().is_err())
}

fn grab_parse_error(message: &str) -> GrabCommand {
    GrabCommand::ParseError(message.to_owned())
}

fn parse_supervisor(args: &[OsString]) -> Result<SupervisorOptions, UsageError> {
    let mut port = 0;
    let mut port_consumed = false;
    let mut journal_override = None;
    let mut no_daily = false;
    let mut no_convey = false;
    let mut no_cortex = false;
    let mut no_spl = false;
    let mut remote = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_os_str() {
            value if value == OsStr::new("--no-daily") => {
                if no_daily {
                    return Err(UsageError);
                }
                no_daily = true;
                index += 1;
            }
            value if value == OsStr::new("--no-convey") => {
                if no_convey {
                    return Err(UsageError);
                }
                no_convey = true;
                index += 1;
            }
            value if value == OsStr::new("--no-cortex") => {
                if no_cortex {
                    return Err(UsageError);
                }
                no_cortex = true;
                index += 1;
            }
            value if value == OsStr::new("--no-spl") => {
                if no_spl {
                    return Err(UsageError);
                }
                no_spl = true;
                index += 1;
            }
            value if value == OsStr::new("--journal") || value == OsStr::new("--remote") => {
                let destination = if value == OsStr::new("--journal") {
                    &mut journal_override
                } else {
                    &mut remote
                };
                if destination.is_some() {
                    return Err(UsageError);
                }
                let value = args.get(index + 1).ok_or(UsageError)?;
                if value.to_string_lossy().starts_with("--") {
                    return Err(UsageError);
                }
                *destination = Some(value.clone());
                index += 2;
            }
            value if !port_consumed => {
                port = value
                    .to_str()
                    .ok_or(UsageError)?
                    .parse()
                    .map_err(|_| UsageError)?;
                port_consumed = true;
                index += 1;
            }
            _ => return Err(UsageError),
        }
    }
    Ok(SupervisorOptions {
        port,
        journal_override,
        no_daily,
        no_convey,
        no_cortex,
        no_spl,
        remote,
    })
}

fn parse_transfer(args: &[OsString]) -> Result<TransferCommand, UsageError> {
    let [verb, rest @ ..] = args else {
        return Err(UsageError);
    };
    match verb.to_str() {
        Some("export") => parse_transfer_export(rest).map(TransferCommand::Export),
        Some("import") => parse_transfer_import(rest).map(TransferCommand::Import),
        Some("send") => parse_transfer_send(rest).map(TransferCommand::Send),
        _ => Err(UsageError),
    }
}

fn parse_transfer_export(args: &[OsString]) -> Result<TransferExportOptions, UsageError> {
    let mut day = None;
    let mut output = None;
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        let destination = match args[index].as_os_str() {
            value if value == OsStr::new("--day") => &mut day,
            value if value == OsStr::new("--output") => &mut output,
            value if value == OsStr::new("--journal") => &mut journal_override,
            _ => return Err(UsageError),
        };
        if destination.is_some() {
            return Err(UsageError);
        }
        let value = args.get(index + 1).ok_or(UsageError)?;
        if value.to_string_lossy().starts_with("--") {
            return Err(UsageError);
        }
        *destination = Some(value.clone());
        index += 2;
    }
    Ok(TransferExportOptions {
        day: day
            .ok_or(UsageError)?
            .into_string()
            .map_err(|_| UsageError)?,
        output: output.ok_or(UsageError)?,
        journal_override,
    })
}

fn parse_transfer_import(args: &[OsString]) -> Result<TransferImportOptions, UsageError> {
    let mut archive = None;
    let mut dry_run = false;
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--dry-run") {
            if dry_run {
                return Err(UsageError);
            }
            dry_run = true;
            index += 1;
            continue;
        }
        let destination = if argument == OsStr::new("--archive") {
            &mut archive
        } else if argument == OsStr::new("--journal") {
            &mut journal_override
        } else {
            return Err(UsageError);
        };
        if destination.is_some() {
            return Err(UsageError);
        }
        let value = args.get(index + 1).ok_or(UsageError)?;
        if value.to_string_lossy().starts_with("--") {
            return Err(UsageError);
        }
        *destination = Some(value.clone());
        index += 2;
    }
    Ok(TransferImportOptions {
        archive: archive.ok_or(UsageError)?,
        dry_run,
        journal_override,
    })
}

fn parse_transfer_send(args: &[OsString]) -> Result<TransferSendOptions, UsageError> {
    let mut to = None;
    let mut day = None;
    let mut dry_run = false;
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--dry-run") {
            if dry_run {
                return Err(UsageError);
            }
            dry_run = true;
            index += 1;
            continue;
        }
        let destination = if argument == OsStr::new("--to") {
            &mut to
        } else if argument == OsStr::new("--day") {
            &mut day
        } else if argument == OsStr::new("--journal") {
            &mut journal_override
        } else {
            return Err(UsageError);
        };
        if destination.is_some() {
            return Err(UsageError);
        }
        let value = args.get(index + 1).ok_or(UsageError)?;
        if value.to_string_lossy().starts_with("--") {
            return Err(UsageError);
        }
        *destination = Some(value.clone());
        index += 2;
    }
    Ok(TransferSendOptions {
        to: to
            .ok_or(UsageError)?
            .into_string()
            .map_err(|_| UsageError)?,
        day: day
            .map(|value| value.into_string().map_err(|_| UsageError))
            .transpose()?,
        dry_run,
        journal_override,
    })
}

fn parse_export(args: &[OsString]) -> Result<ExportOptions, UsageError> {
    let mut to = None;
    let mut only = None;
    let mut day = None;
    let mut key = None;
    let mut journal_override = None;
    let mut dry_run = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--dry-run") {
            if dry_run {
                return Err(UsageError);
            }
            dry_run = true;
            index += 1;
            continue;
        }
        let destination = if argument == OsStr::new("--to") {
            &mut to
        } else if argument == OsStr::new("--only") {
            &mut only
        } else if argument == OsStr::new("--day") {
            &mut day
        } else if argument == OsStr::new("--key") {
            &mut key
        } else if argument == OsStr::new("--journal") {
            &mut journal_override
        } else {
            return Err(UsageError);
        };
        if destination.is_some() {
            return Err(UsageError);
        }
        let value = args.get(index + 1).ok_or(UsageError)?;
        if value.to_string_lossy().starts_with("--") {
            return Err(UsageError);
        }
        *destination = Some(value.clone());
        index += 2;
    }
    Ok(ExportOptions {
        to: to
            .ok_or(UsageError)?
            .into_string()
            .map_err(|_| UsageError)?,
        only: only
            .map(|value| value.into_string().map_err(|_| UsageError))
            .transpose()?,
        day: day
            .map(|value| value.into_string().map_err(|_| UsageError))
            .transpose()?,
        dry_run,
        key: key
            .map(|value| value.into_string().map_err(|_| UsageError))
            .transpose()?,
        journal_override,
    })
}

fn parse_transcribe(args: &[OsString]) -> Result<Command, UsageError> {
    let mut arguments = Vec::with_capacity(args.len());
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        if argument == OsStr::new("--backend") {
            arguments.push(argument.clone().into_string().map_err(|_| UsageError)?);
            if let Some(value) = iter.next() {
                arguments.push(value.clone().into_string().map_err(|_| UsageError)?);
            }
            continue;
        }
        if argument == OsStr::new("-h") || argument == OsStr::new("--help") {
            return Ok(Command::TranscribeHelp);
        }
        if matches!(
            argument.as_os_str(),
            value
                if value == OsStr::new("-v")
                    || value == OsStr::new("--verbose")
                    || value == OsStr::new("-d")
                    || value == OsStr::new("--debug")
        ) {
            continue;
        }
        arguments.push(argument.clone().into_string().map_err(|_| UsageError)?);
    }
    Ok(Command::Transcribe(TranscribeOptions { arguments }))
}

fn parse_facet_candidates(args: &[OsString]) -> Result<(), UsageError> {
    for argument in args {
        match argument.to_str() {
            Some("-v") | Some("--verbose") | Some("-d") | Some("--debug") => {}
            _ => return Err(UsageError),
        }
    }
    Ok(())
}

fn parse_convey(args: &[OsString]) -> Result<ConveyOptions, UsageError> {
    let mut port = None;
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--port") {
            if port.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value == OsStr::new("--port") || value == OsStr::new("--journal") {
                return Err(UsageError);
            }
            port = Some(
                value
                    .to_str()
                    .ok_or(UsageError)?
                    .parse()
                    .map_err(|_| UsageError)?,
            );
            index += 2;
            continue;
        }
        if argument == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value == OsStr::new("--port") || value == OsStr::new("--journal") {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(ConveyOptions {
        port: port.ok_or(UsageError)?,
        journal_override,
    })
}

fn parse_body(args: &[OsString]) -> Result<BodyCommand, UsageError> {
    let [verb, rest @ ..] = args else {
        return Err(UsageError);
    };
    if verb == OsStr::new("apple") {
        return parse_body_apple(rest).map(BodyCommand::Apple);
    }
    if verb == OsStr::new("oura") {
        return parse_body_oura(rest).map(BodyCommand::Oura);
    }
    if verb != OsStr::new("rebuild") {
        return Err(UsageError);
    }
    let mut journal_override = None;
    let mut json = false;
    let mut index = 0;
    while index < rest.len() {
        let argument = rest[index].as_os_str();
        if argument == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
            continue;
        }
        if argument == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = rest.get(index + 1).ok_or(UsageError)?;
            if value == OsStr::new("--json") || value == OsStr::new("--journal") {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(BodyCommand::Rebuild(BodyRebuildOptions {
        journal_override,
        json,
    }))
}

fn parse_body_oura(args: &[OsString]) -> Result<BodyOuraCommand, UsageError> {
    let [verb, rest @ ..] = args else {
        return Err(UsageError);
    };
    if verb == OsStr::new("connect") {
        let (journal_override, json) = parse_body_oura_common(rest)?;
        return Ok(BodyOuraCommand::Connect(BodyOuraConnectOptions {
            journal_override,
            json,
        }));
    }
    if verb != OsStr::new("sync") {
        return Err(UsageError);
    }
    let mut journal_override = None;
    let mut window_days = None;
    let mut save = false;
    let mut confirm_body_save = false;
    let mut scheduled = false;
    let mut json = false;
    let mut index = 0;
    while index < rest.len() {
        let argument = rest[index].as_os_str();
        if argument == OsStr::new("--save") {
            if save {
                return Err(UsageError);
            }
            save = true;
            index += 1;
        } else if argument == OsStr::new("--confirm-body-save") {
            if confirm_body_save {
                return Err(UsageError);
            }
            confirm_body_save = true;
            index += 1;
        } else if argument == OsStr::new("--scheduled") {
            if scheduled {
                return Err(UsageError);
            }
            scheduled = true;
            index += 1;
        } else if argument == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
        } else if argument == OsStr::new("--journal") || argument == OsStr::new("--window-days") {
            let value = rest.get(index + 1).ok_or(UsageError)?;
            if value.to_str().is_some_and(|value| value.starts_with("--")) {
                return Err(UsageError);
            }
            if argument == OsStr::new("--journal") {
                if journal_override.replace(value.clone()).is_some() {
                    return Err(UsageError);
                }
            } else {
                let parsed = value
                    .to_str()
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .ok_or(UsageError)?;
                if window_days.replace(parsed).is_some() {
                    return Err(UsageError);
                }
            }
            index += 2;
        } else {
            return Err(UsageError);
        }
    }
    if (confirm_body_save || scheduled) && !save {
        return Err(UsageError);
    }
    if confirm_body_save && scheduled {
        return Err(UsageError);
    }
    Ok(BodyOuraCommand::Sync(BodyOuraSyncOptions {
        journal_override,
        window_days,
        save,
        confirm_body_save,
        scheduled,
        json,
    }))
}

fn parse_body_oura_common(args: &[OsString]) -> Result<(Option<OsString>, bool), UsageError> {
    let mut journal_override = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
        } else if argument == OsStr::new("--journal") {
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value.to_str().is_some_and(|value| value.starts_with("--"))
                || journal_override.replace(value.clone()).is_some()
            {
                return Err(UsageError);
            }
            index += 2;
        } else {
            return Err(UsageError);
        }
    }
    Ok((journal_override, json))
}

fn parse_body_apple(args: &[OsString]) -> Result<BodyAppleOptions, UsageError> {
    let mut source = None;
    let mut detect = false;
    let mut journal_override = None;
    let mut date_from = None;
    let mut date_to = None;
    let mut save = false;
    let mut confirm_body_save = false;
    let mut force = false;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--detect") {
            if detect {
                return Err(UsageError);
            }
            detect = true;
            index += 1;
        } else if argument == OsStr::new("--save") {
            if save {
                return Err(UsageError);
            }
            save = true;
            index += 1;
        } else if argument == OsStr::new("--confirm-body-save") {
            if confirm_body_save {
                return Err(UsageError);
            }
            confirm_body_save = true;
            index += 1;
        } else if argument == OsStr::new("--force") {
            if force {
                return Err(UsageError);
            }
            force = true;
            index += 1;
        } else if argument == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
        } else if matches!(
            argument,
            value if value == OsStr::new("--source")
                || value == OsStr::new("--journal")
                || value == OsStr::new("--date-from")
                || value == OsStr::new("--date-to")
        ) {
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value.to_str().is_some_and(|value| value.starts_with("--")) {
                return Err(UsageError);
            }
            if argument == OsStr::new("--source") {
                if source.replace(value.clone()).is_some() {
                    return Err(UsageError);
                }
            } else if argument == OsStr::new("--journal") {
                if journal_override.replace(value.clone()).is_some() {
                    return Err(UsageError);
                }
            } else {
                let text = value.to_str().ok_or(UsageError)?.to_owned();
                let slot = if argument == OsStr::new("--date-from") {
                    &mut date_from
                } else {
                    &mut date_to
                };
                if slot.replace(text).is_some() {
                    return Err(UsageError);
                }
            }
            index += 2;
        } else {
            return Err(UsageError);
        }
    }
    if confirm_body_save && !save
        || detect
            && (save
                || confirm_body_save
                || force
                || journal_override.is_some()
                || date_from.is_some()
                || date_to.is_some())
    {
        return Err(UsageError);
    }
    Ok(BodyAppleOptions {
        source: source.ok_or(UsageError)?,
        detect,
        journal_override,
        date_from,
        date_to,
        save,
        confirm_body_save,
        force,
        json,
    })
}

fn parse_speaker_resolve(args: &[OsString]) -> Result<SpeakerResolveCommand, UsageError> {
    match args {
        [command] if command == OsStr::new("accumulate-voiceprints") => {
            Ok(SpeakerResolveCommand::AccumulateVoiceprints)
        }
        [command] if command == OsStr::new("write-owner-centroid") => {
            Ok(SpeakerResolveCommand::WriteOwnerCentroid)
        }
        [command] if command == OsStr::new("rebuild-owner-centroid") => {
            Ok(SpeakerResolveCommand::RebuildOwnerCentroid)
        }
        [command] if command == OsStr::new("write-owner-candidate") => {
            Ok(SpeakerResolveCommand::WriteOwnerCandidate)
        }
        [command] if command == OsStr::new("read-owner-candidate") => {
            Ok(SpeakerResolveCommand::ReadOwnerCandidate)
        }
        [command] if command == OsStr::new("screen-owner-contamination") => {
            Ok(SpeakerResolveCommand::ScreenOwnerContamination)
        }
        [command] if command == OsStr::new("clear-owner-candidate") => {
            Ok(SpeakerResolveCommand::ClearOwnerCandidate)
        }
        [command] if command == OsStr::new("write-voiceprint") => {
            Ok(SpeakerResolveCommand::WriteVoiceprint)
        }
        [command] if command == OsStr::new("remove-voiceprint") => {
            Ok(SpeakerResolveCommand::RemoveVoiceprint)
        }
        [command] if command == OsStr::new("backfill-voiceprint-last-seen") => {
            Ok(SpeakerResolveCommand::BackfillVoiceprintLastSeen)
        }
        [command] if command == OsStr::new("write-stub-labels") => {
            Ok(SpeakerResolveCommand::WriteStubLabels)
        }
        [command] if command == OsStr::new("write-full-labels") => {
            Ok(SpeakerResolveCommand::WriteFullLabels)
        }
        [command] if command == OsStr::new("patch-labels") => {
            Ok(SpeakerResolveCommand::PatchLabels)
        }
        [command] if command == OsStr::new("restore-label-rows") => {
            Ok(SpeakerResolveCommand::RestoreLabelRows)
        }
        [command] if command == OsStr::new("append-correction") => {
            Ok(SpeakerResolveCommand::AppendCorrection)
        }
        [command] if command == OsStr::new("wipe-speaker-artifacts") => {
            Ok(SpeakerResolveCommand::WipeSpeakerArtifacts)
        }
        [command] if command == OsStr::new("identify") => Ok(SpeakerResolveCommand::Identify),
        [command] if command == OsStr::new("undo-identify") => {
            Ok(SpeakerResolveCommand::UndoIdentify)
        }
        [command] if command == OsStr::new("bootstrap-voiceprints") => {
            Ok(SpeakerResolveCommand::BootstrapVoiceprints)
        }
        [command] if command == OsStr::new("seed-from-imports") => {
            Ok(SpeakerResolveCommand::SeedFromImports)
        }
        [command] if command == OsStr::new("merge-names") => Ok(SpeakerResolveCommand::MergeNames),
        [command] if command == OsStr::new("backfill") => Ok(SpeakerResolveCommand::Backfill),
        [command] if command == OsStr::new("backfill-status") => {
            Ok(SpeakerResolveCommand::BackfillStatus)
        }
        _ => Err(UsageError),
    }
}

fn parse_generate(args: &[OsString]) -> GenerateCommand {
    match args {
        [arg] if arg == OsStr::new("--contract") => GenerateCommand::Contract,
        [arg] if arg == OsStr::new("--one-shot") => GenerateCommand::OneShot,
        [command, arguments @ ..] if command == OsStr::new("--session") => {
            GenerateCommand::Session(GenerateSessionOptions {
                arguments: arguments.to_vec(),
            })
        }
        _ => GenerateCommand::Malformed,
    }
}

fn parse_cogitate(args: &[OsString]) -> CogitateCommand {
    match args {
        [arg] if arg == OsStr::new("--contract") => CogitateCommand::Contract,
        [arg] if arg == OsStr::new("--talent-contract") => CogitateCommand::TalentContract,
        [arg] if arg == OsStr::new("--one-shot") => CogitateCommand::OneShot,
        _ => CogitateCommand::Malformed,
    }
}

fn parse_install_models(args: &[OsString]) -> Result<InstallModelsOptions, UsageError> {
    let mut check = false;
    let mut force = false;
    let mut variant = InstallModelsVariant::Auto;
    let mut variant_seen = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--check") {
            if check || force {
                return Err(UsageError);
            }
            check = true;
        } else if argument == OsStr::new("--force") {
            if force || check {
                return Err(UsageError);
            }
            force = true;
        } else {
            let value = if argument == OsStr::new("--variant") {
                index += 1;
                args.get(index).and_then(|value| value.to_str())
            } else {
                argument
                    .to_str()
                    .and_then(|value| value.strip_prefix("--variant="))
            };
            let Some(value) = value else {
                return Err(UsageError);
            };
            if variant_seen {
                return Err(UsageError);
            }
            variant = match value {
                "auto" => InstallModelsVariant::Auto,
                "cpu" => InstallModelsVariant::Cpu,
                "cuda" => InstallModelsVariant::Cuda,
                "coreml" => InstallModelsVariant::Coreml,
                _ => return Err(UsageError),
            };
            variant_seen = true;
        }
        index += 1;
    }
    Ok(InstallModelsOptions {
        check,
        force,
        variant,
    })
}

fn parse_local(args: &[OsString]) -> Result<LocalCommand, UsageError> {
    match args {
        [command] if command == OsStr::new("probe-nvidia") => Ok(LocalCommand::ProbeNvidia),
        [command] if command == OsStr::new("plan") => Ok(LocalCommand::Plan),
        [command] if command == OsStr::new("connect") => Ok(LocalCommand::Connect),
        [command, rest @ ..] if command == OsStr::new("install") => {
            parse_local_install(rest).map(LocalCommand::Install)
        }
        [command] if command == OsStr::new("generate") => Ok(LocalCommand::Generate),
        _ => Err(UsageError),
    }
}

fn parse_local_install(args: &[OsString]) -> Result<InstallCommand, UsageError> {
    match args {
        [one, two] if one == OsStr::new("pins") && two == OsStr::new("local") => {
            Ok(InstallCommand::PinsLocal)
        }
        [one, two] if one == OsStr::new("paths") && two == OsStr::new("local") => {
            Ok(InstallCommand::PathsLocal)
        }
        [one, two] if one == OsStr::new("fingerprint") && two == OsStr::new("local") => {
            Ok(InstallCommand::FingerprintLocal)
        }
        [one, two] if one == OsStr::new("fingerprint") && two == OsStr::new("mlx") => {
            Ok(InstallCommand::FingerprintMlx)
        }
        [one, two] if one == OsStr::new("fingerprint") && two == OsStr::new("parakeet") => {
            Ok(InstallCommand::FingerprintParakeet)
        }
        [one, two] if one == OsStr::new("pins") && two == OsStr::new("parakeet") => {
            Ok(InstallCommand::PinsParakeet)
        }
        [one, two] if one == OsStr::new("paths") && two == OsStr::new("parakeet") => {
            Ok(InstallCommand::PathsParakeet)
        }
        [one, two] if one == OsStr::new("verify") && two == OsStr::new("sha256") => {
            Ok(InstallCommand::VerifySha256)
        }
        [one, two] if one == OsStr::new("cuda") && two == OsStr::new("trust") => {
            Ok(InstallCommand::CudaTrust)
        }
        [one, two] if one == OsStr::new("manifest") && two == OsStr::new("vulkan") => {
            Ok(InstallCommand::ManifestVulkan)
        }
        [one, two] if one == OsStr::new("manifest") && two == OsStr::new("cuda") => {
            Ok(InstallCommand::ManifestCuda)
        }
        [one, two] if one == OsStr::new("manifest") && two == OsStr::new("model") => {
            Ok(InstallCommand::ManifestModel)
        }
        [one, two] if one == OsStr::new("inspect") && two == OsStr::new("local") => {
            Ok(InstallCommand::InspectLocal)
        }
        [one, two] if one == OsStr::new("inspect") && two == OsStr::new("mlx") => {
            Ok(InstallCommand::InspectMlx)
        }
        [one, two] if one == OsStr::new("inspect") && two == OsStr::new("parakeet") => {
            Ok(InstallCommand::InspectParakeet)
        }
        [one] if one == OsStr::new("probe-binary") => Ok(InstallCommand::ProbeBinary),
        [one, two] if one == OsStr::new("run") && two == OsStr::new("local") => {
            Ok(InstallCommand::RunLocal)
        }
        [one, two] if one == OsStr::new("run") && two == OsStr::new("mlx") => {
            Ok(InstallCommand::RunMlx)
        }
        [one, two] if one == OsStr::new("run") && two == OsStr::new("parakeet") => {
            Ok(InstallCommand::RunParakeet)
        }
        _ => Err(UsageError),
    }
}

fn parse_journal_config(args: &[OsString]) -> Result<JournalConfigCommand, UsageError> {
    match args {
        [command, rest @ ..] if command == OsStr::new("read") => {
            parse_journal_config_read(rest).map(JournalConfigCommand::Read)
        }
        [command, rest @ ..] if command == OsStr::new("commit") => {
            parse_journal_config_commit(rest).map(JournalConfigCommand::Commit)
        }
        _ => Err(UsageError),
    }
}

fn parse_brain(args: &[OsString]) -> Result<BrainCommand, UsageError> {
    match args {
        [command, rest @ ..] if command == OsStr::new("refresh") => {
            parse_brain_refresh_session(rest).map(BrainCommand::RefreshSession)
        }
        [command, rest @ ..] if command == OsStr::new("prerequisite-renewal") => {
            parse_brain_prerequisite_renewal_session(rest)
                .map(BrainCommand::PrerequisiteRenewalSession)
        }
        [command, rest @ ..] if command == OsStr::new("record-runtime-failure") => {
            parse_brain_runtime_failure(rest).map(BrainCommand::RecordRuntimeFailure)
        }
        [command, rest @ ..] if command == OsStr::new("inspect") => {
            parse_brain_inspect(rest).map(BrainCommand::Inspect)
        }
        [command, rest @ ..] if command == OsStr::new("fingerprint") => {
            parse_brain_fingerprint(rest).map(|()| BrainCommand::Fingerprint)
        }
        _ => Err(UsageError),
    }
}

fn parse_brain_refresh_session(
    args: &[OsString],
) -> Result<BrainRefreshSessionOptions, UsageError> {
    let mut journal_override = None;
    let mut run_id = None;
    let mut expect = None;
    let mut bundled_runtime_fingerprint_sha256 = None;
    let mut session = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--session") {
            if session {
                return Err(UsageError);
            }
            session = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            journal_override = Some(brain_os_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--run-id") {
            if run_id.is_some() {
                return Err(UsageError);
            }
            run_id = Some(brain_string_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect-fingerprint") {
            if expect.is_some() {
                return Err(UsageError);
            }
            expect = Some(BrainRefreshExpectArg::Sha256(brain_sha256_value(
                args, index,
            )?));
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect-absent") {
            if expect.is_some() {
                return Err(UsageError);
            }
            expect = Some(BrainRefreshExpectArg::Absent);
            index += 1;
            continue;
        }
        if arg == OsStr::new("--bundled-runtime-fingerprint") {
            if bundled_runtime_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            bundled_runtime_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    if !session {
        return Err(UsageError);
    }
    Ok(BrainRefreshSessionOptions {
        journal_override,
        run_id,
        expect,
        bundled_runtime_fingerprint_sha256,
    })
}

fn parse_brain_prerequisite_renewal_session(
    args: &[OsString],
) -> Result<BrainPrerequisiteRenewalSessionOptions, UsageError> {
    let mut journal_override = None;
    let mut run_id = None;
    let mut expected_fingerprint_sha256 = None;
    let mut bundled_runtime_fingerprint_sha256 = None;
    let mut session = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--session") {
            if session {
                return Err(UsageError);
            }
            session = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            journal_override = Some(brain_os_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--run-id") {
            if run_id.is_some() {
                return Err(UsageError);
            }
            run_id = Some(brain_string_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect-fingerprint") {
            if expected_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            expected_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--bundled-runtime-fingerprint") {
            if bundled_runtime_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            bundled_runtime_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    if !session {
        return Err(UsageError);
    }
    Ok(BrainPrerequisiteRenewalSessionOptions {
        journal_override,
        run_id,
        expected_fingerprint_sha256,
        bundled_runtime_fingerprint_sha256,
    })
}

fn parse_brain_runtime_failure(
    args: &[OsString],
) -> Result<BrainRuntimeFailureOptions, UsageError> {
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        if args[index].as_os_str() != OsStr::new("--journal") || journal_override.is_some() {
            return Err(UsageError);
        }
        journal_override = Some(brain_os_value(args, index)?);
        index += 2;
    }
    Ok(BrainRuntimeFailureOptions { journal_override })
}

fn parse_brain_inspect(args: &[OsString]) -> Result<BrainInspectOptions, UsageError> {
    let mut journal_override = None;
    let mut bundled_runtime_fingerprint_sha256 = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            journal_override = Some(brain_os_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--bundled-runtime-fingerprint") {
            if bundled_runtime_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            bundled_runtime_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(BrainInspectOptions {
        journal_override,
        bundled_runtime_fingerprint_sha256,
    })
}

fn parse_brain_fingerprint(args: &[OsString]) -> Result<(), UsageError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(UsageError)
    }
}

fn brain_os_value(args: &[OsString], index: usize) -> Result<OsString, UsageError> {
    let value = args.get(index + 1).ok_or(UsageError)?;
    if value.to_str().is_some_and(|value| value.starts_with('-')) {
        return Err(UsageError);
    }
    Ok(value.clone())
}

fn brain_string_value(args: &[OsString], index: usize) -> Result<String, UsageError> {
    let value = brain_os_value(args, index)?;
    let value = value.into_string().map_err(|_| UsageError)?;
    if value.is_empty() {
        return Err(UsageError);
    }
    Ok(value)
}

fn brain_sha256_value(args: &[OsString], index: usize) -> Result<String, UsageError> {
    let value = brain_string_value(args, index)?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UsageError);
    }
    Ok(value)
}

fn parse_journal_config_read(args: &[OsString]) -> Result<JournalConfigReadOptions, UsageError> {
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(JournalConfigReadOptions { journal_override })
}

fn parse_journal_config_commit(
    args: &[OsString],
) -> Result<JournalConfigCommitOptions, UsageError> {
    let mut journal_override = None;
    let mut lock_timeout_ms = None;
    let mut expect = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        if arg == OsStr::new("--lock-timeout-ms") {
            if lock_timeout_ms.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            let value = value.to_str().ok_or(UsageError)?;
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(UsageError);
            }
            let timeout = value.parse::<u64>().map_err(|_| UsageError)?;
            if timeout == 0 {
                return Err(UsageError);
            }
            lock_timeout_ms = Some(timeout);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect") {
            if expect.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            expect = Some(parse_journal_config_expect(value)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(JournalConfigCommitOptions {
        journal_override,
        lock_timeout_ms,
        expect: expect.ok_or(UsageError)?,
    })
}

fn parse_journal_config_expect(value: &OsString) -> Result<JournalConfigExpectArg, UsageError> {
    if value == OsStr::new("absent") {
        return Ok(JournalConfigExpectArg::Absent);
    }
    let value = value.to_str().ok_or(UsageError)?;
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(UsageError);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UsageError);
    }
    Ok(JournalConfigExpectArg::Sha256(value.to_owned()))
}

fn is_journal_config_flag(value: &OsStr) -> bool {
    matches!(
        value.to_str(),
        Some("--journal" | "--lock-timeout-ms" | "--expect")
    )
}

fn parse_spl(args: &[OsString]) -> Result<SplCommand, UsageError> {
    match args {
        [command, rest @ ..] if command == OsStr::new("service") => {
            parse_service(rest).map(SplCommand::Service)
        }
        _ => Err(UsageError),
    }
}

fn parse_service(args: &[OsString]) -> Result<ServiceOptions, UsageError> {
    let mut verbose = false;
    let mut debug = false;
    for argument in args {
        let argument = argument.as_os_str();
        if argument == OsStr::new("-v") || argument == OsStr::new("--verbose") {
            if verbose {
                return Err(UsageError);
            }
            verbose = true;
            continue;
        }
        if argument == OsStr::new("-d") || argument == OsStr::new("--debug") {
            if debug {
                return Err(UsageError);
            }
            debug = true;
            continue;
        }
        return Err(UsageError);
    }
    Ok(ServiceOptions { verbose, debug })
}

fn parse_journal_path(args: &[OsString]) -> Result<JournalPathOptions, UsageError> {
    let mut journal_override = None;
    let mut create = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--create") {
            if create {
                return Err(UsageError);
            }
            create = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value == OsStr::new("--create") || value == OsStr::new("--journal") {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(JournalPathOptions {
        journal_override,
        create,
    })
}

fn parse_indexer(args: &[OsString]) -> Result<IndexerCommand, UsageError> {
    match args {
        [verb, rest @ ..] if verb == OsStr::new("search") => {
            parse_indexer_search(rest).map(IndexerCommand::Search)
        }
        [verb, rest @ ..] if verb == OsStr::new("counts") => {
            parse_indexer_counts(rest).map(IndexerCommand::Counts)
        }
        [verb, rest @ ..] if verb == OsStr::new("agents") => {
            parse_indexer_read(rest).map(IndexerCommand::Agents)
        }
        [verb, rest @ ..] if verb == OsStr::new("coverage") => {
            parse_indexer_read(rest).map(IndexerCommand::Coverage)
        }
        [verb, rest @ ..] if verb == OsStr::new("prune-stream") => {
            parse_indexer_prune_stream(rest).map(IndexerCommand::PruneStream)
        }
        [verb, rest @ ..] if verb == OsStr::new("prune-paths") => {
            parse_indexer_prune_paths(rest).map(IndexerCommand::PrunePaths)
        }
        [verb, rest @ ..] if verb == OsStr::new("fold-entity-edges") => {
            parse_indexer_fold_entity_edges(rest).map(IndexerCommand::FoldEntityEdges)
        }
        [verb, rest @ ..] if verb == OsStr::new("edge-fingerprint") => {
            parse_indexer_read(rest).map(IndexerCommand::EdgeFingerprint)
        }
        [verb, rest @ ..] if verb == OsStr::new("rebuild-edges-fingerprint") => {
            parse_indexer_read(rest).map(IndexerCommand::RebuildEdgesFingerprint)
        }
        _ => parse_indexer_maintenance(args).map(IndexerCommand::Maintenance),
    }
}

fn parse_indexer_maintenance(args: &[OsString]) -> Result<IndexerOptions, UsageError> {
    let mut journal_override = None;
    let mut json = false;
    let mut reset = false;
    let mut rebuild_edges = false;
    let mut rescan = false;
    let mut rescan_full = false;
    let mut rescan_file = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--reset") {
            if reset {
                return Err(UsageError);
            }
            reset = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--rebuild-edges") {
            if rebuild_edges {
                return Err(UsageError);
            }
            rebuild_edges = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--rescan") {
            if rescan {
                return Err(UsageError);
            }
            rescan = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--rescan-full") {
            if rescan_full {
                return Err(UsageError);
            }
            rescan_full = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_maintenance_indexer_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        if arg == OsStr::new("--rescan-file") {
            if rescan_file.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_maintenance_indexer_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            rescan_file = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }

    if rescan_file.is_some() && (rescan || rescan_full) {
        return Err(UsageError);
    }

    Ok(IndexerOptions {
        journal_override,
        json,
        reset,
        rebuild_edges,
        rescan,
        rescan_full,
        rescan_file,
    })
}

fn is_maintenance_indexer_flag(value: &OsStr) -> bool {
    matches!(
        value.to_str(),
        Some(
            "--journal"
                | "--json"
                | "--reset"
                | "--rebuild-edges"
                | "--rescan"
                | "--rescan-full"
                | "--rescan-file",
        )
    )
}

fn parse_indexer_prune_stream(args: &[OsString]) -> Result<IndexerPruneStreamOptions, UsageError> {
    let (values, journal_override, json) = parse_indexer_values(args, 1, Some(1))?;
    Ok(IndexerPruneStreamOptions {
        journal_override,
        json,
        stream: values[0].clone(),
    })
}

fn parse_indexer_prune_paths(args: &[OsString]) -> Result<IndexerPrunePathsOptions, UsageError> {
    let (paths, journal_override, json) = parse_indexer_values(args, 1, None)?;
    Ok(IndexerPrunePathsOptions {
        journal_override,
        json,
        paths,
    })
}

fn parse_indexer_fold_entity_edges(
    args: &[OsString],
) -> Result<IndexerFoldEntityEdgesOptions, UsageError> {
    let (values, journal_override, json) = parse_indexer_values(args, 2, Some(2))?;
    Ok(IndexerFoldEntityEdgesOptions {
        journal_override,
        json,
        source_id: values[0].clone(),
        target_id: values[1].clone(),
    })
}

fn parse_indexer_values(
    args: &[OsString],
    minimum: usize,
    maximum: Option<usize>,
) -> Result<(Vec<String>, Option<OsString>, bool), UsageError> {
    let mut values = Vec::new();
    let mut journal_override = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
        } else if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            journal_override = Some(args.get(index + 1).ok_or(UsageError)?.clone());
            index += 2;
        } else {
            let value = arg.to_str().ok_or(UsageError)?;
            if value.starts_with("--") {
                return Err(UsageError);
            }
            values.push(value.to_string());
            index += 1;
        }
    }
    if values.len() < minimum || maximum.is_some_and(|limit| values.len() > limit) {
        return Err(UsageError);
    }
    Ok((values, journal_override, json))
}

fn parse_indexer_search(args: &[OsString]) -> Result<IndexerSearchOptions, UsageError> {
    let parsed = parse_indexer_query(args, true)?;
    Ok(IndexerSearchOptions {
        query: parsed.query,
        limit: parsed.limit,
        offset: parsed.offset,
        counts: parsed.counts,
        order: parsed.order,
    })
}

fn parse_indexer_counts(args: &[OsString]) -> Result<IndexerCountsOptions, UsageError> {
    let parsed = parse_indexer_query(args, false)?;
    Ok(IndexerCountsOptions {
        query: parsed.query,
    })
}

fn parse_indexer_read(args: &[OsString]) -> Result<IndexerReadOptions, UsageError> {
    let mut journal_override = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_query_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(IndexerReadOptions {
        journal_override,
        json,
    })
}

struct ParsedIndexerQuery {
    query: IndexerQueryOptions,
    limit: usize,
    offset: usize,
    counts: bool,
    order: String,
}

fn parse_indexer_query(
    args: &[OsString],
    allow_search_options: bool,
) -> Result<ParsedIndexerQuery, UsageError> {
    let mut query = None;
    let mut journal_override = None;
    let mut json = false;
    let mut day = None;
    let mut day_from = None;
    let mut day_to = None;
    let mut facet = None;
    let mut agent = None;
    let mut stream = None;
    let mut time_bucket = None;
    let mut relax = false;
    let mut limit = 10;
    let mut offset = 0;
    let mut counts = false;
    let mut order = "relevance".to_string();
    let mut limit_seen = false;
    let mut offset_seen = false;
    let mut order_seen = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--relax") {
            if relax {
                return Err(UsageError);
            }
            relax = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_query_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        let string_slot = match arg.to_str() {
            Some("--day") => Some(&mut day),
            Some("--day-from") => Some(&mut day_from),
            Some("--day-to") => Some(&mut day_to),
            Some("--facet") => Some(&mut facet),
            Some("--agent") => Some(&mut agent),
            Some("--stream") => Some(&mut stream),
            Some("--time-bucket") => Some(&mut time_bucket),
            _ => None,
        };
        if let Some(slot) = string_slot {
            if slot.is_some() {
                return Err(UsageError);
            }
            let value = query_value(args, index)?;
            *slot = Some(value);
            index += 2;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--limit") {
            if limit_seen {
                return Err(UsageError);
            }
            limit = parse_usize_option(args, index)?;
            limit_seen = true;
            index += 2;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--offset") {
            if offset_seen {
                return Err(UsageError);
            }
            offset = parse_usize_option(args, index)?;
            offset_seen = true;
            index += 2;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--counts") {
            if counts {
                return Err(UsageError);
            }
            counts = true;
            index += 1;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--order") {
            if order_seen {
                return Err(UsageError);
            }
            order = query_value(args, index)?;
            order_seen = true;
            index += 2;
            continue;
        }
        if arg.to_str().is_some_and(|value| value.starts_with('-')) || query.is_some() {
            return Err(UsageError);
        }
        query = Some(arg.to_str().ok_or(UsageError)?.to_string());
        index += 1;
    }
    Ok(ParsedIndexerQuery {
        query: IndexerQueryOptions {
            journal_override,
            json,
            query,
            day,
            day_from,
            day_to,
            facet,
            agent,
            stream,
            time_bucket,
            relax,
        },
        limit,
        offset,
        counts,
        order,
    })
}

fn query_value(args: &[OsString], index: usize) -> Result<String, UsageError> {
    let value = args.get(index + 1).ok_or(UsageError)?;
    if is_query_flag(value.as_os_str()) {
        return Err(UsageError);
    }
    value.to_str().map(str::to_string).ok_or(UsageError)
}

fn parse_usize_option(args: &[OsString], index: usize) -> Result<usize, UsageError> {
    query_value(args, index)?.parse().map_err(|_| UsageError)
}

fn is_query_flag(value: &OsStr) -> bool {
    value.to_str().is_some_and(|value| value.starts_with('-'))
}

pub fn version_line(version: &str) -> String {
    format!("solstone-core {version}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn indexer(command: IndexerCommand) -> Command {
        Command::Indexer(Box::new(command))
    }

    #[test]
    fn accepts_version_flag() {
        assert_eq!(evaluate_args(&args(&["--version"])), Ok(Command::Version));
    }

    #[test]
    fn accepts_assets_command_without_arguments() {
        assert_eq!(evaluate_args(&args(&["assets"])), Ok(Command::Assets));
        assert_eq!(
            evaluate_args(&args(&["assets", "unexpected"])),
            Err(UsageError)
        );
    }

    #[test]
    fn rejects_empty_args() {
        assert_eq!(evaluate_args(&args(&[])), Err(UsageError));
    }

    #[test]
    fn parses_navigate_arguments_in_either_option_position() {
        assert_eq!(
            evaluate_args(&args(&["navigate"])),
            Ok(Command::Navigate {
                path: None,
                facet: None,
            })
        );
        for (values, expected_path, expected_facet) in [
            (
                &["navigate", "/home", "--facet", "work"][..],
                Some("/home"),
                Some("work"),
            ),
            (
                &["navigate", "--facet", "work", "/home"][..],
                Some("/home"),
                Some("work"),
            ),
            (
                &["navigate", "-f", "work", "/home"][..],
                Some("/home"),
                Some("work"),
            ),
            (
                &["navigate", "--facet=work", "/a"][..],
                Some("/a"),
                Some("work"),
            ),
            (
                &["navigate", "/a", "--facet=work"][..],
                Some("/a"),
                Some("work"),
            ),
            (&["navigate", "-fwork", "/a"][..], Some("/a"), Some("work")),
            (&["navigate", "--facet=-x"][..], None, Some("-x")),
            (&["navigate", "--facet="][..], None, Some("")),
            (&["navigate", "-f=work"][..], None, Some("=work")),
            (&["navigate", "--", "-weird"][..], Some("-weird"), None),
            (&["navigate", "--", "--help"][..], Some("--help"), None),
            (&["navigate", "--", "-h"][..], Some("-h"), None),
            (
                &["navigate", "--", "--facet=work"][..],
                Some("--facet=work"),
                None,
            ),
            (
                &["navigate", "-f", "work", "--", "-weird"][..],
                Some("-weird"),
                Some("work"),
            ),
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Navigate {
                    path: expected_path.map(str::to_owned),
                    facet: expected_facet.map(str::to_owned),
                }),
                "{values:?}"
            );
        }
        assert_eq!(
            evaluate_args(&args(&["navigate", "--facet", ""])),
            Ok(Command::Navigate {
                path: None,
                facet: Some(String::new()),
            })
        );
        assert_eq!(
            evaluate_args(&args(&["navigate", "--"])),
            Ok(Command::Navigate {
                path: None,
                facet: None,
            })
        );
    }

    #[test]
    fn navigate_help_is_a_carrier_before_parsing() {
        for values in [
            &["navigate", "--help"][..],
            &["navigate", "-h"][..],
            &["navigate", "/home", "--help"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Ok(Command::NavigateHelp));
        }
    }

    #[test]
    fn navigate_malformed_arguments_are_usage_carriers() {
        for values in [
            &["navigate", "--facet"][..],
            &["navigate", "-f"][..],
            &["navigate", "--nonsense"][..],
            &["navigate", "-x"][..],
            &["navigate", "-weird"][..],
            &["navigate", "/a", "/b"][..],
            &["navigate", "--facet", "one", "-f", "two"][..],
            &["navigate", "--facet", "--nonsense"][..],
            &["navigate", "--facet", "--"][..],
            &["navigate", "-f", "-x"][..],
            &["navigate", "--facet=a", "--facet=b"][..],
            &["navigate", "--facet=a", "-f", "b"][..],
            &["navigate", "-fa", "--facet", "b"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::NavigateUsage),
                "{values:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn navigate_non_utf8_arguments_are_usage_carriers() {
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            evaluate_args(&[OsString::from("navigate"), OsString::from_vec(vec![0xff]),]),
            Ok(Command::NavigateUsage)
        );
    }

    #[test]
    fn parses_identity_grammar_with_click_value_spellings() {
        assert_eq!(
            evaluate_args(&args(&["identity"])),
            Ok(Command::Identity(IdentityCommand::Hydrate))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "identity",
                "partner",
                "--write",
                "--value=first",
                "--value",
                "last",
            ])),
            Ok(Command::Identity(IdentityCommand::Partner(
                IdentityPartnerOptions {
                    write: true,
                    update_section: None,
                    value: Some("last".to_owned()),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "identity",
                "partner",
                "--update-section=H",
                "--value=-x",
            ])),
            Ok(Command::Identity(IdentityCommand::Partner(
                IdentityPartnerOptions {
                    write: false,
                    update_section: Some("H".to_owned()),
                    value: Some("-x".to_owned()),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["identity", "briefing", "-d20260101"])),
            Ok(Command::Identity(IdentityCommand::Briefing(
                IdentityBriefingOptions {
                    day: Some("20260101".to_owned()),
                }
            )))
        );
    }

    #[test]
    fn identity_help_and_usage_are_scope_specific_carriers() {
        for (values, expected) in [
            (&["identity", "--help"][..], Command::IdentityHelp),
            (
                &["identity", "partner", "-h"][..],
                Command::IdentityPartnerHelp,
            ),
            (
                &["identity", "health", "--help"][..],
                Command::IdentityHealthHelp,
            ),
            (
                &["identity", "briefing", "-h"][..],
                Command::IdentityBriefingHelp,
            ),
            (
                &["identity", "unknown"][..],
                Command::IdentityUnknownCommand("unknown".to_owned()),
            ),
            (
                &["identity", "partner", "--value"][..],
                Command::IdentityPartnerUsage,
            ),
            (
                &["identity", "health", "--refresh=yes"][..],
                Command::IdentityHealthUsage,
            ),
            (
                &["identity", "briefing", "--day", "bad"][..],
                Command::IdentityBriefingUsage,
            ),
        ] {
            assert_eq!(evaluate_args(&args(values)), Ok(expected), "{values:?}");
        }
    }

    #[test]
    fn rejects_unknown_args() {
        assert_eq!(evaluate_args(&args(&["--unknown"])), Err(UsageError));
    }

    #[test]
    fn transcribe_intercepts_help_and_discards_logging_flags() {
        assert_eq!(
            evaluate_args(&args(&[
                "transcribe",
                "--all",
                "-v",
                "--debug",
                "--backend",
                "parakeet",
            ])),
            Ok(Command::Transcribe(TranscribeOptions {
                arguments: vec![
                    "--all".to_owned(),
                    "--backend".to_owned(),
                    "parakeet".to_owned(),
                ],
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["transcribe", "--nonsense", "-h"])),
            Ok(Command::TranscribeHelp)
        );
        assert_eq!(
            evaluate_args(&args(&[
                "transcribe",
                "--backend",
                "--debug",
                "parakeet",
                "--all",
            ])),
            Ok(Command::Transcribe(TranscribeOptions {
                arguments: vec![
                    "--backend".to_owned(),
                    "--debug".to_owned(),
                    "parakeet".to_owned(),
                    "--all".to_owned(),
                ],
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["transcribe", "--backend", "-h", "--all"])),
            Ok(Command::Transcribe(TranscribeOptions {
                arguments: vec!["--backend".to_owned(), "-h".to_owned(), "--all".to_owned()],
            }))
        );
    }

    #[test]
    fn rejects_extra_args() {
        assert_eq!(
            evaluate_args(&args(&["--version", "extra"])),
            Err(UsageError)
        );
    }

    #[test]
    fn parses_body_rebuild_options_and_rejects_ambiguous_forms() {
        assert_eq!(
            evaluate_args(&args(&["body", "rebuild"])),
            Ok(Command::Body(BodyCommand::Rebuild(BodyRebuildOptions {
                journal_override: None,
                json: false,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "body",
                "rebuild",
                "--json",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::Body(BodyCommand::Rebuild(BodyRebuildOptions {
                journal_override: Some("/tmp/journal".into()),
                json: true,
            })))
        );
        for values in [
            &["body"][..],
            &["body", "unknown"][..],
            &["body", "rebuild", "--journal"][..],
            &["body", "rebuild", "--journal", "--json"][..],
            &["body", "rebuild", "--json", "--json"][..],
            &["body", "rebuild", "--journal", "/one", "--journal", "/two"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn parses_body_apple_preview_and_save_without_ambiguous_flags() {
        assert_eq!(
            evaluate_args(&args(&[
                "body",
                "apple",
                "--source",
                "/tmp/export.zip",
                "--date-from",
                "2026-01-01",
                "--json",
            ])),
            Ok(Command::Body(BodyCommand::Apple(BodyAppleOptions {
                source: "/tmp/export.zip".into(),
                detect: false,
                journal_override: None,
                date_from: Some("2026-01-01".to_owned()),
                date_to: None,
                save: false,
                confirm_body_save: false,
                force: false,
                json: true,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "body",
                "apple",
                "--source",
                "/tmp/export",
                "--journal",
                "/tmp/journal",
                "--save",
                "--confirm-body-save",
            ])),
            Ok(Command::Body(BodyCommand::Apple(BodyAppleOptions {
                source: "/tmp/export".into(),
                detect: false,
                journal_override: Some("/tmp/journal".into()),
                date_from: None,
                date_to: None,
                save: true,
                confirm_body_save: true,
                force: false,
                json: false,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "body",
                "apple",
                "--source",
                "/tmp/export.zip",
                "--detect",
                "--json",
            ])),
            Ok(Command::Body(BodyCommand::Apple(BodyAppleOptions {
                source: "/tmp/export.zip".into(),
                detect: true,
                journal_override: None,
                date_from: None,
                date_to: None,
                save: false,
                confirm_body_save: false,
                force: false,
                json: true,
            })))
        );
        for values in [
            &["body", "apple"][..],
            &["body", "apple", "--source"][..],
            &["body", "apple", "--source", "--save"][..],
            &[
                "body",
                "apple",
                "--source",
                "/tmp/export",
                "--confirm-body-save",
            ][..],
            &["body", "apple", "--source", "/one", "--source", "/two"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn parses_body_oura_connect_and_sync_without_ambiguous_authority() {
        assert_eq!(
            evaluate_args(&args(&[
                "body",
                "oura",
                "connect",
                "--journal",
                "/tmp/journal",
                "--json",
            ])),
            Ok(Command::Body(BodyCommand::Oura(BodyOuraCommand::Connect(
                BodyOuraConnectOptions {
                    journal_override: Some("/tmp/journal".into()),
                    json: true,
                }
            ))))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "body",
                "oura",
                "sync",
                "--journal",
                "/tmp/journal",
                "--window-days",
                "14",
                "--save",
                "--confirm-body-save",
                "--json",
            ])),
            Ok(Command::Body(BodyCommand::Oura(BodyOuraCommand::Sync(
                BodyOuraSyncOptions {
                    journal_override: Some("/tmp/journal".into()),
                    window_days: Some(14),
                    save: true,
                    confirm_body_save: true,
                    scheduled: false,
                    json: true,
                }
            ))))
        );
        assert_eq!(
            evaluate_args(&args(&["body", "oura", "sync", "--save", "--scheduled",])),
            Ok(Command::Body(BodyCommand::Oura(BodyOuraCommand::Sync(
                BodyOuraSyncOptions {
                    journal_override: None,
                    window_days: None,
                    save: true,
                    confirm_body_save: false,
                    scheduled: true,
                    json: false,
                }
            ))))
        );
        for values in [
            &["body", "oura"][..],
            &["body", "oura", "connect", "--window-days", "7"][..],
            &["body", "oura", "sync", "--window-days", "0"][..],
            &["body", "oura", "sync", "--confirm-body-save"][..],
            &["body", "oura", "sync", "--scheduled"][..],
            &[
                "body",
                "oura",
                "sync",
                "--save",
                "--scheduled",
                "--confirm-body-save",
            ][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn parses_convey_options_and_rejects_invalid_ports() {
        assert_eq!(
            evaluate_args(&args(&[
                "convey",
                "--port",
                "5015",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::Convey(ConveyOptions {
                port: 5015,
                journal_override: Some("/tmp/journal".into()),
            }))
        );
        for values in [
            &["convey"][..],
            &["convey", "--port"][..],
            &["convey", "--port", "not-a-port"][..],
            &["convey", "--port", "65536"][..],
            &["convey", "--port", "5015", "--port", "5016"][..],
            &["convey", "--journal", "/tmp/journal", "--port", "--journal"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_local_probe_nvidia() {
        assert_eq!(
            evaluate_args(&args(&["local", "probe-nvidia"])),
            Ok(Command::Local(LocalCommand::ProbeNvidia))
        );
    }

    #[test]
    fn accepts_local_plan_connect_and_generate() {
        assert_eq!(
            evaluate_args(&args(&["local", "plan"])),
            Ok(Command::Local(LocalCommand::Plan))
        );
        assert_eq!(
            evaluate_args(&args(&["local", "connect"])),
            Ok(Command::Local(LocalCommand::Connect))
        );
        assert_eq!(
            evaluate_args(&args(&["local", "generate"])),
            Ok(Command::Local(LocalCommand::Generate))
        );
    }

    #[test]
    fn accepts_local_install_parakeet_verbs() {
        for (args_values, expected) in [
            (
                &["local", "install", "pins", "parakeet"][..],
                InstallCommand::PinsParakeet,
            ),
            (
                &["local", "install", "paths", "parakeet"][..],
                InstallCommand::PathsParakeet,
            ),
            (
                &["local", "install", "fingerprint", "parakeet"][..],
                InstallCommand::FingerprintParakeet,
            ),
            (
                &["local", "install", "inspect", "parakeet"][..],
                InstallCommand::InspectParakeet,
            ),
            (
                &["local", "install", "run", "parakeet"][..],
                InstallCommand::RunParakeet,
            ),
        ] {
            assert_eq!(
                evaluate_args(&args(args_values)),
                Ok(Command::Local(LocalCommand::Install(expected))),
                "{args_values:?}"
            );
        }
    }

    #[test]
    fn classifies_generate_arguments_without_usage_errors() {
        for (values, expected) in [
            (&["generate", "--contract"][..], GenerateCommand::Contract),
            (&["generate", "--one-shot"][..], GenerateCommand::OneShot),
            (
                &["generate", "--session", "--max-in-flight", "3"][..],
                GenerateCommand::Session(GenerateSessionOptions {
                    arguments: vec!["--max-in-flight".into(), "3".into()],
                }),
            ),
            (
                &["generate", "--session"][..],
                GenerateCommand::Session(GenerateSessionOptions { arguments: vec![] }),
            ),
            (
                &["generate", "--session", "--max-in-flight"][..],
                GenerateCommand::Session(GenerateSessionOptions {
                    arguments: vec!["--max-in-flight".into()],
                }),
            ),
            (&["generate"][..], GenerateCommand::Malformed),
            (&["generate", "--bogus"][..], GenerateCommand::Malformed),
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Generate(expected)),
                "{values:?}"
            );
        }
    }

    #[test]
    fn classifies_cogitate_arguments_without_usage_errors() {
        for (values, expected) in [
            (&["cogitate", "--contract"][..], CogitateCommand::Contract),
            (
                &["cogitate", "--talent-contract"][..],
                CogitateCommand::TalentContract,
            ),
            (&["cogitate", "--one-shot"][..], CogitateCommand::OneShot),
            (&["cogitate"][..], CogitateCommand::Malformed),
            (&["cogitate", "--bogus"][..], CogitateCommand::Malformed),
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Cogitate(expected)),
                "{values:?}"
            );
        }
    }

    #[test]
    fn rejects_unimplemented_or_extra_local_args() {
        for values in [&["local"][..], &["local", "probe-nvidia", "extra"][..]] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_journal_path() {
        assert_eq!(
            evaluate_args(&args(&["journal-path"])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: None,
                create: false,
            }))
        );
    }

    #[test]
    fn accepts_indexer_without_operation_flags() {
        assert_eq!(
            evaluate_args(&args(&["indexer"])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: None,
                json: false,
                reset: false,
                rebuild_edges: false,
                rescan: false,
                rescan_full: false,
                rescan_file: None,
            })))
        );
    }

    #[test]
    fn accepts_indexer_search_with_filters_and_options() {
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "search",
                "needle",
                "--journal",
                "/tmp/journal",
                "--json",
                "--limit",
                "12",
                "--offset",
                "3",
                "--day-from",
                "20260101",
                "--agent",
                "flow",
                "--relax",
                "--counts",
                "--order",
                "recency",
            ])),
            Ok(indexer(IndexerCommand::Search(IndexerSearchOptions {
                query: IndexerQueryOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                    json: true,
                    query: Some("needle".to_string()),
                    day: None,
                    day_from: Some("20260101".to_string()),
                    day_to: None,
                    facet: None,
                    agent: Some("flow".to_string()),
                    stream: None,
                    time_bucket: None,
                    relax: true,
                },
                limit: 12,
                offset: 3,
                counts: true,
                order: "recency".to_string(),
            })))
        );
    }

    #[test]
    fn accepts_indexer_counts_and_read_verbs() {
        assert_eq!(
            evaluate_args(&args(&["indexer", "counts", "--facet", "work"])),
            Ok(indexer(IndexerCommand::Counts(IndexerCountsOptions {
                query: IndexerQueryOptions {
                    journal_override: None,
                    json: false,
                    query: None,
                    day: None,
                    day_from: None,
                    day_to: None,
                    facet: Some("work".to_string()),
                    agent: None,
                    stream: None,
                    time_bucket: None,
                    relax: false,
                },
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["indexer", "agents", "--json"])),
            Ok(indexer(IndexerCommand::Agents(IndexerReadOptions {
                journal_override: None,
                json: true,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["indexer", "coverage"])),
            Ok(indexer(IndexerCommand::Coverage(IndexerReadOptions {
                journal_override: None,
                json: false,
            })))
        );
    }

    #[test]
    fn accepts_journal_config_read_and_commit() {
        assert_eq!(
            evaluate_args(&args(&[
                "journal-config",
                "read",
                "--journal",
                "/tmp/journal"
            ])),
            Ok(Command::JournalConfig(JournalConfigCommand::Read(
                JournalConfigReadOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "journal-config",
                "commit",
                "--lock-timeout-ms",
                "25",
                "--expect",
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::JournalConfig(JournalConfigCommand::Commit(
                JournalConfigCommitOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                    lock_timeout_ms: Some(25),
                    expect: JournalConfigExpectArg::Sha256(
                        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                            .to_owned()
                    ),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["journal-config", "commit", "--expect", "absent"])),
            Ok(Command::JournalConfig(JournalConfigCommand::Commit(
                JournalConfigCommitOptions {
                    journal_override: None,
                    lock_timeout_ms: None,
                    expect: JournalConfigExpectArg::Absent,
                }
            )))
        );
    }

    #[test]
    fn accepts_brain_verbs() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "refresh",
                "--session",
                "--journal",
                "/tmp/journal",
                "--run-id",
                "run-1",
                "--expect-fingerprint",
                hash,
                "--bundled-runtime-fingerprint",
                hash,
            ])),
            Ok(Command::Brain(BrainCommand::RefreshSession(
                BrainRefreshSessionOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                    run_id: Some("run-1".to_owned()),
                    expect: Some(BrainRefreshExpectArg::Sha256(hash.to_owned())),
                    bundled_runtime_fingerprint_sha256: Some(hash.to_owned()),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(
                &["brain", "refresh", "--expect-absent", "--session",]
            )),
            Ok(Command::Brain(BrainCommand::RefreshSession(
                BrainRefreshSessionOptions {
                    journal_override: None,
                    run_id: None,
                    expect: Some(BrainRefreshExpectArg::Absent),
                    bundled_runtime_fingerprint_sha256: None,
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--run-id",
                "renew-1",
                "--expect-fingerprint",
                hash,
                "--bundled-runtime-fingerprint",
                hash,
            ])),
            Ok(Command::Brain(BrainCommand::PrerequisiteRenewalSession(
                BrainPrerequisiteRenewalSessionOptions {
                    journal_override: None,
                    run_id: Some("renew-1".to_owned()),
                    expected_fingerprint_sha256: Some(hash.to_owned()),
                    bundled_runtime_fingerprint_sha256: Some(hash.to_owned()),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "record-runtime-failure",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::Brain(BrainCommand::RecordRuntimeFailure(
                BrainRuntimeFailureOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "inspect",
                "--journal",
                "/tmp/journal",
                "--bundled-runtime-fingerprint",
                hash,
            ])),
            Ok(Command::Brain(BrainCommand::Inspect(BrainInspectOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                bundled_runtime_fingerprint_sha256: Some(hash.to_owned()),
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["brain", "inspect"])),
            Ok(Command::Brain(BrainCommand::Inspect(BrainInspectOptions {
                journal_override: None,
                bundled_runtime_fingerprint_sha256: None,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["brain", "fingerprint"])),
            Ok(Command::Brain(BrainCommand::Fingerprint))
        );
    }

    #[test]
    fn rejects_invalid_brain_args() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        for values in [
            &["brain"][..],
            &["brain", "unknown"][..],
            &["brain", "refresh"][..],
            &["brain", "prerequisite-renewal"][..],
            &[
                "brain",
                "refresh",
                "--session",
                "--expect-fingerprint",
                hash,
                "--expect-absent",
            ][..],
            &["brain", "refresh", "--session", "--session"][..],
            &["brain", "refresh", "--session", "--run-id"][..],
            &[
                "brain",
                "refresh",
                "--session",
                "--journal",
                "--run-id",
                "id",
            ][..],
            &[
                "brain",
                "refresh",
                "--session",
                "--bundled-runtime-fingerprint",
                "bad",
            ][..],
            &[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--expect-absent",
            ][..],
            &[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--expect-fingerprint",
                "bad",
            ][..],
            &[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--expect-fingerprint",
                hash,
                "--expect-fingerprint",
                hash,
            ][..],
            &["brain", "record-runtime-failure", "--journal"][..],
            &["brain", "inspect", "--unknown"][..],
            &["brain", "fingerprint", "--journal", "/tmp/journal"][..],
            &["brain", "inspect", "--journal"][..],
            &["brain", "inspect", "--bundled-runtime-fingerprint", "bad"][..],
            &["brain", "inspect", "--journal", "/a", "--journal", "/b"][..],
            &[
                "brain",
                "record-runtime-failure",
                "--journal",
                "/a",
                "--journal",
                "/b",
            ][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_indexer_verb_unknown_duplicate_and_disallowed_options() {
        for values in [
            &["indexer", "search", "--limit", "10", "--limit", "10"][..],
            &["indexer", "search", "needle", "second"][..],
            &["indexer", "counts", "--limit", "10"][..],
            &["indexer", "agents", "needle"][..],
            &["indexer", "coverage", "--day", "20260101"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_invalid_journal_config_args() {
        for values in [
            &["journal-config"][..],
            &["journal-config", "unknown"][..],
            &["journal-config", "read", "--expect", "absent"][..],
            &["journal-config", "read", "--journal"][..],
            &["journal-config", "read", "--journal", "--expect"][..],
            &["journal-config", "commit"][..],
            &["journal-config", "commit", "--expect"][..],
            &["journal-config", "commit", "--expect", "bogus"][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--expect",
                "absent",
            ][..],
            &[
                "journal-config",
                "commit",
                "--journal",
                "/a",
                "--journal",
                "/b",
                "--expect",
                "absent",
            ][..],
            &[
                "journal-config",
                "commit",
                "--lock-timeout-ms",
                "10",
                "--lock-timeout-ms",
                "20",
                "--expect",
                "absent",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "0",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "-1",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "1ms",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "18446744073709551616",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--journal",
                "--lock-timeout-ms",
            ][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_spl_service() {
        assert_eq!(
            evaluate_args(&args(&["spl", "service"])),
            Ok(Command::Spl(SplCommand::Service(ServiceOptions {
                verbose: false,
                debug: false,
            })))
        );
    }

    #[test]
    fn accepts_spl_service_verbose_and_debug_flags_in_either_order() {
        assert_eq!(
            evaluate_args(&args(&["spl", "service", "-v", "--debug"])),
            Ok(Command::Spl(SplCommand::Service(ServiceOptions {
                verbose: true,
                debug: true,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["spl", "service", "-d", "--verbose"])),
            Ok(Command::Spl(SplCommand::Service(ServiceOptions {
                verbose: true,
                debug: true,
            })))
        );
    }

    #[test]
    fn accepts_each_spl_service_flag() {
        for (flag, expected) in [
            (
                "-v",
                ServiceOptions {
                    verbose: true,
                    debug: false,
                },
            ),
            (
                "--verbose",
                ServiceOptions {
                    verbose: true,
                    debug: false,
                },
            ),
            (
                "-d",
                ServiceOptions {
                    verbose: false,
                    debug: true,
                },
            ),
            (
                "--debug",
                ServiceOptions {
                    verbose: false,
                    debug: true,
                },
            ),
        ] {
            assert_eq!(
                evaluate_args(&args(&["spl", "service", flag])),
                Ok(Command::Spl(SplCommand::Service(expected))),
                "{flag}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_or_unknown_spl_service_flags() {
        for values in [
            &["spl", "service", "-v", "-v"][..],
            &["spl", "service", "--verbose", "--verbose"][..],
            &["spl", "service", "-v", "--verbose"][..],
            &["spl", "service", "-d", "-d"][..],
            &["spl", "service", "--debug", "--debug"][..],
            &["spl", "service", "-d", "--debug"][..],
            &["spl", "service", "--unknown"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_spl_service_extra_args() {
        for values in [
            &["spl", "service", "extra"][..],
            &["spl", "service", "service"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_incomplete_unknown_and_extra_spl_args() {
        for values in [&["spl"][..], &["spl", "unknown"][..]] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_indexer_rescan_full_reset_and_override() {
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "--journal",
                "/tmp/journal",
                "--reset",
                "--rescan-full",
            ])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                json: false,
                reset: true,
                rebuild_edges: false,
                rescan: false,
                rescan_full: true,
                rescan_file: None,
            })))
        );
    }

    #[test]
    fn accepts_indexer_rescan_file() {
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "--rescan-file",
                "20240101/talents/flow.md",
            ])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: None,
                json: false,
                reset: false,
                rebuild_edges: false,
                rescan: false,
                rescan_full: false,
                rescan_file: Some(OsString::from("20240101/talents/flow.md")),
            })))
        );
    }

    #[test]
    fn accepts_indexer_rebuild_edges_composed_with_rescan() {
        assert_eq!(
            evaluate_args(&args(&["indexer", "--rebuild-edges", "--rescan"])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: None,
                json: false,
                reset: false,
                rebuild_edges: true,
                rescan: true,
                rescan_full: false,
                rescan_file: None,
            })))
        );
    }

    #[test]
    fn accepts_native_indexer_mutation_verbs() {
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "prune-stream",
                "private",
                "--journal",
                "/tmp/journal",
                "--json",
            ])),
            Ok(indexer(IndexerCommand::PruneStream(
                IndexerPruneStreamOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                    json: true,
                    stream: "private".to_string(),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "prune-paths",
                "20260809/default/090000_300",
                "legacy/path.md",
                "--json",
            ])),
            Ok(indexer(IndexerCommand::PrunePaths(
                IndexerPrunePathsOptions {
                    journal_override: None,
                    json: true,
                    paths: vec![
                        "20260809/default/090000_300".to_string(),
                        "legacy/path.md".to_string(),
                    ],
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "fold-entity-edges",
                "source",
                "target",
                "--json",
            ])),
            Ok(indexer(IndexerCommand::FoldEntityEdges(
                IndexerFoldEntityEdgesOptions {
                    journal_override: None,
                    json: true,
                    source_id: "source".to_string(),
                    target_id: "target".to_string(),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["indexer", "edge-fingerprint", "--json"])),
            Ok(indexer(IndexerCommand::EdgeFingerprint(
                IndexerReadOptions {
                    journal_override: None,
                    json: true,
                }
            )))
        );
    }

    #[test]
    fn rejects_malformed_native_indexer_mutations() {
        for values in [
            &["indexer", "prune-stream"][..],
            &["indexer", "prune-stream", "one", "two"][..],
            &["indexer", "prune-paths"][..],
            &["indexer", "fold-entity-edges", "source"][..],
            &["indexer", "fold-entity-edges", "source", "target", "extra"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_indexer_conflicts_missing_values_and_duplicates() {
        for values in [
            &["indexer", "--rescan-file"][..],
            &["indexer", "--journal"][..],
            &["indexer", "--rescan-file", "--rescan"][..],
            &["indexer", "--journal", "--reset"][..],
            &["indexer", "--reset", "--reset"][..],
            &["indexer", "--rebuild-edges", "--rebuild-edges"][..],
            &["indexer", "--rescan-file", "a.md", "--rescan"][..],
            &["indexer", "--rescan-file", "a.md", "--rescan-full"][..],
            &["indexer", "--unknown"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_journal_path_create() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--create"])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: None,
                create: true,
            }))
        );
    }

    #[test]
    fn accepts_journal_path_override() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--journal", "/tmp/journal"])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                create: false,
            }))
        );
    }

    #[test]
    fn accepts_journal_path_override_create() {
        assert_eq!(
            evaluate_args(&args(&[
                "journal-path",
                "--journal",
                "/tmp/journal",
                "--create",
            ])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                create: true,
            }))
        );
    }

    #[test]
    fn accepts_journal_path_create_override() {
        assert_eq!(
            evaluate_args(&args(&[
                "journal-path",
                "--create",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                create: true,
            }))
        );
    }

    #[test]
    fn rejects_journal_missing_value() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--journal"])),
            Err(UsageError)
        );
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--journal", "--create"])),
            Err(UsageError)
        );
    }

    #[test]
    fn rejects_journal_path_unknown_flags() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--unknown"])),
            Err(UsageError)
        );
    }

    #[test]
    fn rejects_journal_path_duplicate_flags() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--create", "--create"])),
            Err(UsageError)
        );
        assert_eq!(
            evaluate_args(&args(&[
                "journal-path",
                "--journal",
                "/a",
                "--journal",
                "/b",
            ])),
            Err(UsageError)
        );
    }

    #[test]
    fn parses_speaker_transcript_write_without_arguments() {
        assert_eq!(
            evaluate_args(&args(&["speaker-transcript-write"])),
            Ok(Command::SpeakerTranscriptWrite)
        );
        assert_eq!(
            evaluate_args(&args(&["speaker-transcript-write", "--redo"])),
            Err(UsageError)
        );
    }

    #[test]
    fn parses_speaker_resolve_verbs() {
        for (verb, expected) in [
            ("write-stub-labels", SpeakerResolveCommand::WriteStubLabels),
            ("write-full-labels", SpeakerResolveCommand::WriteFullLabels),
            ("patch-labels", SpeakerResolveCommand::PatchLabels),
            (
                "restore-label-rows",
                SpeakerResolveCommand::RestoreLabelRows,
            ),
            ("append-correction", SpeakerResolveCommand::AppendCorrection),
            (
                "backfill-voiceprint-last-seen",
                SpeakerResolveCommand::BackfillVoiceprintLastSeen,
            ),
            ("write-voiceprint", SpeakerResolveCommand::WriteVoiceprint),
            ("remove-voiceprint", SpeakerResolveCommand::RemoveVoiceprint),
            (
                "wipe-speaker-artifacts",
                SpeakerResolveCommand::WipeSpeakerArtifacts,
            ),
            (
                "clear-owner-candidate",
                SpeakerResolveCommand::ClearOwnerCandidate,
            ),
            (
                "screen-owner-contamination",
                SpeakerResolveCommand::ScreenOwnerContamination,
            ),
            ("identify", SpeakerResolveCommand::Identify),
            ("undo-identify", SpeakerResolveCommand::UndoIdentify),
            (
                "bootstrap-voiceprints",
                SpeakerResolveCommand::BootstrapVoiceprints,
            ),
            ("seed-from-imports", SpeakerResolveCommand::SeedFromImports),
            ("merge-names", SpeakerResolveCommand::MergeNames),
            ("backfill", SpeakerResolveCommand::Backfill),
            ("backfill-status", SpeakerResolveCommand::BackfillStatus),
        ] {
            assert_eq!(
                evaluate_args(&args(&["speaker-resolve", verb])),
                Ok(Command::SpeakerResolve(expected)),
            );
        }
    }

    #[test]
    fn formats_version_line() {
        assert_eq!(version_line("1.2.3"), "solstone-core 1.2.3\n");
    }

    #[test]
    fn usage_lists_supported_commands() {
        // This was a frozen copy of USAGE and had already drifted from it --
        // red on main before this lane touched it, because a snapshot of prose
        // goes stale every time the prose is correctly edited. Assert the
        // invariant instead: every command the binary dispatches is listed.
        for command in [
            "--version",
            "assets",
            "warm",
            "check",
            "journal-path",
            "indexer",
            "journal-config",
            "speaker-transcript-write",
            "observer",
            "speaker-resolve",
            "local",
            "generate",
            "cogitate",
            "brain",
            "body",
            "transfer",
            "convey",
            "grab",
            "spl",
            "supervisor",
            "navigate",
            "identity",
            "settings",
            "export",
            "facet-candidates",
            "install-models",
            "streams",
            "segment",
            "journal-stats",
            "reprocess",
            "backfill-processing-records",
        ] {
            assert!(
                USAGE.contains(&format!("solstone-core {command}")),
                "USAGE does not list `{command}`"
            );
        }
        assert!(USAGE.starts_with("Usage:\n"));
    }

    #[test]
    fn parses_facet_candidates_arguments() {
        for values in [
            &["facet-candidates"][..],
            &["facet-candidates", "-v"][..],
            &["facet-candidates", "--verbose"][..],
            &["facet-candidates", "-d"][..],
            &["facet-candidates", "--debug"][..],
            &["facet-candidates", "-d", "-v"][..],
            &["facet-candidates", "--verbose", "--debug"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::FacetCandidates),
                "{values:?}"
            );
        }

        for values in [
            &["facet-candidates", "--help"][..],
            &["facet-candidates", "-h"][..],
            &["facet-candidates", "-v", "--help"][..],
            &["facet-candidates", "-d", "-h"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::FacetCandidatesHelp),
                "{values:?}"
            );
        }

        for values in [
            &["facet-candidates", "--nonsense"][..],
            &["facet-candidates", "extra-positional"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::FacetCandidatesUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parses_install_models_options_and_rejects_conflicts() {
        assert_eq!(
            evaluate_args(&args(&["install-models"])),
            Ok(Command::InstallModels(InstallModelsOptions {
                check: false,
                force: false,
                variant: InstallModelsVariant::Auto,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["install-models", "--check", "--variant=cpu"])),
            Ok(Command::InstallModels(InstallModelsOptions {
                check: true,
                force: false,
                variant: InstallModelsVariant::Cpu,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["install-models", "--force", "--variant", "coreml"])),
            Ok(Command::InstallModels(InstallModelsOptions {
                check: false,
                force: true,
                variant: InstallModelsVariant::Coreml,
            }))
        );
        for values in [
            &["install-models", "--check", "--force"][..],
            &["install-models", "--variant"][..],
            &["install-models", "--variant", "bad"][..],
            &["install-models", "--variant", "cpu", "--variant", "cuda"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn parses_transfer_send_options_and_rejects_ambiguous_forms() {
        assert_eq!(
            evaluate_args(&args(&[
                "transfer",
                "send",
                "--to",
                "office",
                "--day",
                "20260203-20260204",
                "--dry-run",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::Transfer(TransferCommand::Send(
                TransferSendOptions {
                    to: "office".to_string(),
                    day: Some("20260203-20260204".to_string()),
                    dry_run: true,
                    journal_override: Some("/tmp/journal".into()),
                }
            )))
        );
        for values in [
            &["transfer", "send"][..],
            &["transfer", "send", "--to"][..],
            &["transfer", "send", "--to", "office", "--to", "home"][..],
            &["transfer", "send", "--to", "office", "--day", "--dry-run"][..],
            &[
                "transfer",
                "send",
                "--to",
                "office",
                "--dry-run",
                "--dry-run",
            ][..],
            &["transfer", "send", "--to", "office", "--unknown"][..],
        ] {
            // A transfer parse failure is deliberately NOT `Err(UsageError)`:
            // that path exits 64 with solstone-core's usage, where the reference
            // exits 2 with `journal transfer`'s. Rejection is still rejection --
            // it is carried as a command so main can render it faithfully.
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::TransferUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn grab_usage_is_frozen() {
        assert_eq!(
            GRAB_USAGE,
            "usage: journal grab [-h] [--out OUT] [--force] [--json] [-v] [-d] [args ...]\n"
        );
    }

    #[test]
    fn accepts_grab_zero_through_many_positional_tokens() {
        for count in 0..=7 {
            let mut values = vec!["grab"];
            let expected: Vec<_> = (0..count).map(|index| format!("token-{index}")).collect();
            values.extend(expected.iter().map(String::as_str));
            assert_eq!(
                evaluate_args(&args(&values)),
                Ok(Command::Grab(GrabCommand::Run(GrabOptions {
                    tokens: expected,
                    out: None,
                    force: false,
                    json: false,
                    verbose: false,
                    debug: false,
                }))),
                "{count} tokens"
            );
        }
    }

    #[test]
    fn accepts_grab_option_forms_duplicates_and_contiguous_trailing_options() {
        for values in [
            &[
                "grab",
                "--out",
                "frame.png",
                "--force",
                "--json",
                "-v",
                "--verbose",
                "-d",
                "--debug",
                "DAY",
                "STREAM",
            ][..],
            &[
                "grab",
                "--json",
                "DAY",
                "STREAM",
                "--out=frame.png",
                "--force",
            ][..],
            &["grab", "--json", "--json", "DAY"][..],
        ] {
            assert!(matches!(
                evaluate_args(&args(values)),
                Ok(Command::Grab(GrabCommand::Run(_)))
            ));
        }
        assert_eq!(
            evaluate_args(&args(&["grab", "--out=frame.png"])),
            Ok(Command::Grab(GrabCommand::Run(GrabOptions {
                tokens: vec![],
                out: Some(OsString::from("frame.png")),
                force: false,
                json: false,
                verbose: false,
                debug: false,
            })))
        );
    }

    #[test]
    fn accepts_each_grab_boolean_option_individually() {
        for (flag, force, json, verbose, debug) in [
            ("--force", true, false, false, false),
            ("--json", false, true, false, false),
            ("-v", false, false, true, false),
            ("--verbose", false, false, true, false),
            ("-d", false, false, false, true),
            ("--debug", false, false, false, true),
        ] {
            assert_eq!(
                evaluate_args(&args(&["grab", flag])),
                Ok(Command::Grab(GrabCommand::Run(GrabOptions {
                    tokens: vec![],
                    out: None,
                    force,
                    json,
                    verbose,
                    debug,
                }))),
                "{flag}"
            );
        }
    }

    #[test]
    fn grab_end_of_options_and_help_short_circuit() {
        assert_eq!(
            evaluate_args(&args(&["grab", "--", "--weird-day-name"])),
            Ok(Command::Grab(GrabCommand::Run(GrabOptions {
                tokens: vec!["--weird-day-name".to_owned()],
                out: None,
                force: false,
                json: false,
                verbose: false,
                debug: false,
            })))
        );
        for values in [
            &["grab", "--help"][..],
            &["grab", "DAY", "-h", "STREAM"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Grab(GrabCommand::Help))
            );
        }
    }

    #[test]
    fn rejects_grab_malformed_and_interspersed_argv() {
        assert_eq!(
            evaluate_args(&args(&["grab", "DAY", "--json", "STREAM"])),
            Ok(Command::Grab(GrabCommand::ParseError(
                "unrecognized arguments: STREAM".to_owned()
            )))
        );
        for values in [
            &["grab", "--bogus"][..],
            &["grab", "--out"][..],
            &["grab", "--out", "--json"][..],
            &["grab", "DAY", "--json", "--", "STREAM"][..],
        ] {
            assert!(
                matches!(
                    evaluate_args(&args(values)),
                    Ok(Command::Grab(GrabCommand::ParseError(_)))
                ),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parses_supervisor_journal_override() {
        assert_eq!(
            evaluate_args(&args(&["supervisor", "--journal", "/tmp/journal"])),
            Ok(Command::Supervisor(SupervisorOptions {
                port: 0,
                journal_override: Some(OsString::from("/tmp/journal")),
                no_daily: false,
                no_convey: false,
                no_cortex: false,
                no_spl: false,
                remote: None,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["supervisor", "--journal"])),
            Err(UsageError)
        );
        assert_eq!(
            evaluate_args(&args(&["supervisor", "--wat"])),
            Err(UsageError)
        );
    }

    #[test]
    fn parses_supervisor_stack_options_in_any_order() {
        assert_eq!(
            evaluate_args(&args(&[
                "supervisor",
                "5015",
                "--no-spl",
                "--remote",
                "https://example.test",
                "--journal",
                "/tmp/journal",
                "--no-convey",
                "--no-cortex",
                "--no-daily",
            ])),
            Ok(Command::Supervisor(SupervisorOptions {
                port: 5015,
                journal_override: Some(OsString::from("/tmp/journal")),
                no_daily: true,
                no_convey: true,
                no_cortex: true,
                no_spl: true,
                remote: Some(OsString::from("https://example.test")),
            }))
        );
    }

    #[test]
    fn rejects_invalid_supervisor_stack_options() {
        for values in [
            &["supervisor", "--no-convey", "--no-convey"][..],
            &["supervisor", "--no-cortex", "--no-cortex"][..],
            &["supervisor", "--no-spl", "--no-spl"][..],
            &["supervisor", "--no-daily", "--no-daily"][..],
            &["supervisor", "--remote"][..],
            &["supervisor", "--remote", "--no-spl"][..],
            &["supervisor", "--remote", "a", "--remote", "b"][..],
            &["supervisor", "--journal", "/a", "--journal", "/b"][..],
            &["supervisor", "5015", "6015"][..],
            &["supervisor", "nope-a-port"][..],
            &["supervisor", "--unknown"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }
}
