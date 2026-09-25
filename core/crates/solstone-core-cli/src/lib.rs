// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use solstone_core_format::segment::segment_key;

pub use solstone_core_cli_boundary::DESCRIBE_USAGE;

#[cfg(test)]
#[path = "../../solstone-core-system-health/tests/support/fixtures.rs"]
mod health_text_fixture;

macro_rules! speaker_resolve_usage {
    () => {
        "  solstone-core speaker-resolve <accumulate-voiceprints|write-owner-centroid|rebuild-owner-centroid|write-owner-candidate|read-owner-candidate|screen-owner-contamination|clear-owner-candidate|write-voiceprint|remove-voiceprint|backfill-voiceprint-last-seen|write-stub-labels|write-full-labels|patch-labels|restore-label-rows|append-correction|wipe-speaker-artifacts|identify|undo-identify|bootstrap-voiceprints|seed-from-imports|merge-names|backfill|backfill-status>\n"
    };
}

pub const USAGE: &str = concat!(
    "Usage:\n  solstone-core --version\n  solstone-core warm [--json]\n  solstone-core check [--json]\n  solstone-core assets\n  solstone-core doctor [--verbose] [--json | --jsonl] [--port PORT] [--feature NAME] [--readiness]\n  solstone-core journal-path [--journal PATH] [--create]\n  solstone-core indexer [--journal PATH] [--reset] [--rebuild-edges] [--rescan | --rescan-full | --rescan-file PATH]\n  solstone-core indexer search [QUERY] [--journal PATH] [--json] [--limit N] [--offset N] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax] [--counts] [--order relevance|recency]\n  solstone-core indexer counts [QUERY] [--journal PATH] [--json] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax]\n  solstone-core indexer agents [--journal PATH] [--json]\n  solstone-core indexer coverage [--journal PATH] [--json]\n  solstone-core journal-config read [--journal PATH]\n  solstone-core journal-config commit [--journal PATH] [--lock-timeout-ms N] --expect <fingerprint|absent>\n  solstone-core speaker-transcript-write\n",
    speaker_resolve_usage!(),
    "  solstone-core local probe-nvidia\n  solstone-core local plan\n  solstone-core local connect\n  solstone-core local install <pins|paths|fingerprint|verify|cuda|manifest|inspect|probe-binary|run> ...\n  solstone-core local generate\n  solstone-core generate --contract\n  solstone-core generate --one-shot\n  solstone-core generate --session --max-in-flight N\n  solstone-core cogitate --contract\n  solstone-core cogitate --talent-contract\n  solstone-core cogitate --one-shot\n  solstone-core brain refresh --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256 | --expect-absent] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain prerequisite-renewal --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain record-runtime-failure [--journal PATH]\n  solstone-core brain inspect [--journal PATH] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain fingerprint\n  solstone-core body rebuild [--journal PATH] [--json]\n  solstone-core body apple --source PATH [--detect | [--journal PATH] [--date-from DAY] [--date-to DAY] [--force] [--save [--confirm-body-save]] [--json]\n  solstone-core body oura connect [--journal PATH] [--json]\n  solstone-core body oura sync [--journal PATH] [--window-days N] [--save [--confirm-body-save | --scheduled]] [--json]\n  journal convey --port PORT [--journal PATH]\n  journal schedule [-v | --verbose] [-d | --debug]\n  solstone-core grab [DAY [STREAM [SEGMENT [SCREEN [FRAME_ID[,FRAME_ID...]]]]]] [--out PATH] [--force] [--json] [-v | --verbose] [-d | --debug] [-h | --help]\n  solstone-core spl service [-v | --verbose] [-d | --debug]\n  solstone-core supervisor [PORT] [--direct-port DIRECT_PORT] [--no-daily] [--journal PATH] [--no-convey] [--no-cortex] [--no-spl] [--no-schedule]\n",
    "  journal top [-h] [-v | --verbose] [-d | --debug]\n  journal health [-h] [-v | --verbose] [-d | --debug]\n  journal health logs [-h] [-c N] [-f] [--since TIME] [--service NAME] [--grep PATTERN] [-v | --verbose] [-d | --debug]\n",
    "  solstone-core sense [-v | --verbose] [-d | --debug]\n",
    "  solstone-core navigate [-h | --help] PATH\n",
    "  solstone-core identity [-h | --help] <partner|health|briefing> ...\n",
    "  solstone-core settings [-h | --help] [-v | --verbose] [-d | --debug] [convey [status [--json]]]\n",
    "  solstone-core contract <build|check> ...\n",
    "  solstone-core transcribe [-h] [--all] [--redo] [--backend {parakeet,parakeet-cpp,confidential}] [-v] [-d] [audio_path]\n",
    "  solstone-core facet-candidates [-h] [-v] [-d]\n  solstone-core install-models [--check | --force] [--variant {auto,cpu,cuda,coreml}]\n  solstone-core install-provider <name>\n  solstone-core thinking set-lane {local,byo,confidential} [--provider PROVIDER] [--model MODEL] [--journal PATH]\n",
    "  solstone-core streams [args...]\n",
    "  solstone-core importer [args...]\n",
    "  solstone-core segment [args...]\n",
    "  solstone-core backup [args...]\n",
    "  solstone-core journal-stats [args...]\n",
    "  solstone-core talent [args...]\n",
    "  solstone-core reprocess [args...]\n",
    "  solstone-core backfill-processing-records [args...]\n"
);

pub const SPEAKER_RESOLVE_USAGE: &str = speaker_resolve_usage!();
pub const THINK_USAGE: &str = "usage: journal think [-h] [--day DAY] [--segment SEGMENT] [--refresh] [--from-scratch] [--segments] [--facet NAME] [--activity ID] [--reactivate] [--stream STREAM] [--flush] [-j N] [--no-timeout] [--segment-workers N] [--no-activity-prompts] [--skip-talents SKIP_TALENTS] [--live] [--updated] [--weekly] [--cadence] [--dry-run] [--sense-batch] [-v] [-d]\n";
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
pub const NAVIGATE_USAGE: &str = "usage: journal navigate [-h] PATH\n";

/// Owner-facing grammar for the deterministic heartbeat pass.
pub const HEARTBEAT_USAGE: &str = "usage: journal heartbeat [-h] [--force]\n";

pub const HEARTBEAT_HELP: &str = concat!(
    "usage: journal heartbeat [-h] [--force]\n",
    "\n",
    "Run deterministic health repair pass\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --force     Run full check regardless of recency\n",
);

pub const ENGAGE_USAGE: &str =
    "usage: journal engage [-h] [--wait] [--facet FACET] [--day DAY] NAME\n";

pub const ENGAGE_HELP: &str = concat!(
    "usage: journal engage [-h] [--wait] [--facet FACET] [--day DAY] NAME\n",
    "\n",
    "Delegate work to a cogitate agent.\n",
    "\n",
    "positional arguments:\n",
    "  NAME           Agent name to delegate to (e.g. partner).\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  --wait         Block until the agent completes and print its result.\n",
    "  --facet FACET  Facet context for the agent.\n",
    "  --day DAY      Day context for the agent (e.g. 20260404).\n",
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngageOptions {
    pub name: String,
    pub wait: bool,
    pub facet: Option<String>,
    pub day: Option<String>,
}

pub const JOURNAL_BRAIN_OWNER_SENTINEL: &str = "\u{1f}solstone-journal-brain-owner-v1";
pub const BRAIN_OWNER_USAGE: &str = "usage: journal brain [-h] {status,refresh} ...\n";
pub const BRAIN_OWNER_HELP: &str = concat!(
    "usage: journal brain [-h] {status,refresh} ...\n\n",
    "Active-brain status and bounded refresh CLI.\n\n",
    "positional arguments:\n  {status,refresh}\n    status              Show active-brain status\n    refresh             Run one bounded active-brain check\n\n",
    "options:\n  -h, --help          show this help message and exit\n",
);
pub const BRAIN_STATUS_HELP: &str = concat!(
    "usage: journal brain status [-h] [--json]\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --json      Emit JSON instead of plain output\n",
);
pub const BRAIN_REFRESH_HELP: &str = concat!(
    "usage: journal brain refresh [-h] [--json] [--expected-fingerprint EXPECTED_FINGERPRINT]\n",
    "                            [--expected-active-fingerprint]\n",
    "                            [--expect-active-fingerprint-absent]\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --json                Emit JSON instead of plain output\n",
    "  --expected-fingerprint EXPECTED_FINGERPRINT\n",
    "  --expected-active-fingerprint\n",
    "  --expect-active-fingerprint-absent\n",
);
pub const BRAIN_RENEW_PREREQUISITES_HELP: &str = concat!(
    "usage: journal brain renew-prerequisites [-h]\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
);

/// `journal navigate --help` in the owner-facing command vocabulary.
/// It names `journal navigate`, not `solstone-core navigate`, because that is
/// the command the owner typed.
pub const NAVIGATE_HELP: &str = concat!(
    "usage: journal navigate [-h] PATH\n",
    "\n",
    "Navigate the browser to a path.\n",
    "\n",
    "positional arguments:\n",
    "  PATH                  URL path to navigate to.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
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

pub const CONTRACT_USAGE: &str = "usage: journal contract [-h] {build,check} ...\n";
pub const CONTRACT_HELP: &str = concat!(
    "usage: journal contract [-h] {build,check} ...\n\n",
    "Build and validate the journal contract bundle.\n\n",
    "positional arguments:\n  {build,check}\n",
    "    build               Build the contract bundle.\n",
    "    check               Check the bundle and journal files.\n\n",
    "options:\n  -h, --help            show this help message and exit\n",
);
pub const CONTRACT_BUILD_USAGE: &str =
    "usage: journal contract build [-h] [--check] [--root PATH]\n";
pub const CONTRACT_BUILD_HELP: &str = concat!(
    "usage: journal contract build [-h] [--check] [--root PATH]\n\n",
    "Build the journal contract bundle.\n\n",
    "options:\n  -h, --help            show this help message and exit\n",
    "  --check               Check whether the bundle is current without writing.\n",
    "  --root PATH           Contract checkout or installed-package root.\n",
);
pub const CONTRACT_CHECK_USAGE: &str =
    "usage: journal contract check [-h] [--journal PATH]... [--root PATH]\n";
pub const CONTRACT_CHECK_HELP: &str = concat!(
    "usage: journal contract check [-h] [--journal PATH]... [--root PATH]\n\n",
    "Check the bundle and journal files against their schemas.\n\n",
    "options:\n  -h, --help            show this help message and exit\n",
    "  --journal PATH        Additional journal root to validate (repeatable).\n",
    "  --root PATH           Contract checkout or installed-package root.\n",
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

pub const SUPERVISOR_USAGE: &str = concat!(
    "usage: journal supervisor [-h] [--no-daily] [--no-cortex] [--no-spl]\n",
    "                          [--no-convey] [--no-schedule]\n",
    "                          [--journal JOURNAL] [--direct-port DIRECT_PORT]\n",
    "                          [-v] [-d]\n",
    "                          [port]\n",
);

pub const START_USAGE: &str = concat!(
    "usage: journal start [-h] [--no-daily] [--no-cortex] [--no-spl]\n",
    "                     [--no-convey] [--no-schedule]\n",
    "                     [--hosted-parent] [--journal JOURNAL]\n",
    "                     [--direct-port DIRECT_PORT] [-v] [-d]\n",
    "                     [port]\n",
);

pub const SUPERVISOR_HELP: &str = concat!(
    "usage: journal supervisor [-h] [--no-daily] [--no-cortex] [--no-spl]\n",
    "                          [--no-convey] [--no-schedule]\n",
    "                          [--journal JOURNAL] [--direct-port DIRECT_PORT]\n",
    "                          [-v] [-d]\n",
    "                          [port]\n",
    "\n",
    "Monitor journal system health\n",
    "\n",
    "positional arguments:\n",
    "  port               Convey port (0 = auto-select available port)\n",
    "\n",
    "options:\n",
    "  -h, --help         show this help message and exit\n",
    "  --no-daily         Disable daily processing run at midnight\n",
    "  --no-cortex        Do not start the Cortex server (run it manually for\n",
    "                     debugging)\n",
    "  --no-spl           Do not start the private network relay\n",
    "  --no-convey        Do not start the Convey web application\n",
    "  --no-schedule      Do not initialize or run the schedule engine\n",
    "  --journal JOURNAL  Use this path as the journal root instead of normal\n",
    "                     journal resolution.\n",
    "  --direct-port DIRECT_PORT\n",
    "                     Paired-device door port (default: 7657)\n",
    "  -v, --verbose      Enable verbose output\n",
    "  -d, --debug        Enable debug logging\n",
);

pub const START_HELP: &str = concat!(
    "usage: journal start [-h] [--no-daily] [--no-cortex] [--no-spl]\n",
    "                     [--no-convey] [--no-schedule]\n",
    "                     [--hosted-parent] [--journal JOURNAL]\n",
    "                     [--direct-port DIRECT_PORT] [-v] [-d]\n",
    "                     [port]\n",
    "\n",
    "Monitor journal system health\n",
    "\n",
    "positional arguments:\n",
    "  port               Convey port (0 = auto-select available port)\n",
    "\n",
    "options:\n",
    "  -h, --help         show this help message and exit\n",
    "  --no-daily         Disable daily processing run at midnight\n",
    "  --no-cortex        Do not start the Cortex server (run it manually for\n",
    "                     debugging)\n",
    "  --no-spl           Do not start the private network relay\n",
    "  --no-convey        Do not start the Convey web application\n",
    "  --no-schedule      Do not initialize or run the schedule engine\n",
    "  --hosted-parent    Require the direct parent process to remain live\n",
    "  --journal JOURNAL  Use this path as the journal root instead of normal\n",
    "                     journal resolution.\n",
    "  --direct-port DIRECT_PORT\n",
    "                     Paired-device door port (default: 7657)\n",
    "  -v, --verbose      Enable verbose output\n",
    "  -d, --debug        Enable debug logging\n",
);

pub const HEALTH_USAGE: &str = "usage: journal health [-h] [-v] [-d]\n";

pub const TOP_USAGE: &str = "usage: journal top [-h] [-v] [-d]\n";

pub const TOP_HELP: &str = concat!(
    "usage: journal top [-h] [-v] [-d]\n",
    "\nShow interactive service, observation, task, and brain activity.\n\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  -v, --verbose  enable verbose output\n",
    "  -d, --debug    enable debug output\n",
);

pub const UP_HELP: &str = concat!(
    "usage: journal up [-h]\n",
    "\n",
    "Start the installed journal service if it is not running, then wait until it is ready.\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
);

pub const DOWN_HELP: &str = concat!(
    "usage: journal down [-h]\n",
    "\n",
    "Stop the journal service.\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
);

pub const HEALTH_HELP: &str = concat!(
    "usage: journal health [-h] [-v] [-d]\n",
    "\n",
    "Show the retained supervisor service-health status.\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  -v, --verbose  enable verbose output\n",
    "  -d, --debug    enable debug output\n",
);

pub const HEALTH_LOGS_USAGE: &str = "usage: journal health logs [-h] [-c N] [-f] [--since TIME] [--service NAME] [--grep PATTERN] [-v] [-d]\n";

pub const HEALTH_LOGS_HELP: &str = concat!(
    "usage: journal health logs [-h] [-c N] [-f] [--since TIME] [--service NAME] [--grep PATTERN] [-v] [-d]\n",
    "\nView operational service logs.\n\noptions:\n",
    "  -h, --help            show this help message and exit\n",
    "  -c N                  number of lines to show (default: 5)\n",
    "  -f                    follow log output\n",
    "  --since TIME          show rows at or after TIME\n",
    "  --service NAME        show rows for one service\n",
    "  --grep PATTERN        show rows matching PATTERN\n",
    "  -v, --verbose         enable verbose output\n",
    "  -d, --debug           enable debug output\n",
);

pub const SENSE_USAGE: &str = concat!(
    "usage: journal sense [-h] [--day DAY] [-j JOBS]\n",
    "                    [--reprocess {screen,audio,all}] [--segment SEGMENT]\n",
    "                    [--stream STREAM] [--dry-run] [-v] [-d]\n",
);

pub const SENSE_HELP: &str = concat!(
    "usage: journal sense [-h] [--day DAY] [-j JOBS]\n",
    "                    [--reprocess {screen,audio,all}] [--segment SEGMENT]\n",
    "                    [--stream STREAM] [--dry-run] [-v] [-d]\n",
    "\n",
    "Unified observe file processor\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --day DAY             Process files from specific day (YYYYMMDD format) instead of watching\n",
    "  -j JOBS, --jobs JOBS  Max concurrent screen-describe jobs when using --day (default: 1).\n",
    "  --reprocess {screen,audio,all}\n",
    "                        Delete existing outputs and reprocess (requires --day)\n",
    "  --segment SEGMENT     Filter to specific segment (HHMMSS_LEN format, requires --day)\n",
    "  --stream STREAM       Filter to specific stream (requires --day)\n",
    "  --dry-run             Show what would be processed (or deleted with --reprocess) without making changes\n",
    "  -v, --verbose         Enable verbose output\n",
    "  -d, --debug           Enable debug logging\n",
);

pub const CORTEX_USAGE: &str = "usage: journal cortex [-h] [-v] [-d]\n";

pub const CORTEX_HELP: &str = concat!(
    "usage: journal cortex [-h] [-v] [-d]\n",
    "\n",
    "solstone Cortex Talent Manager\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

pub const CHECK_USAGE: &str = "usage: journal check [-h] [--json]\n";

pub const CHECK_HELP: &str = concat!(
    "usage: journal check [-h] [--json]\n",
    "\n",
    "Readiness verdict for bundled local journal models.\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --json      emit the readiness verdict as JSON for agents\n",
);

pub const INSTALL_MODELS_USAGE: &str = concat!(
    "usage: journal install-models [-h] [--check | --force]\n",
    "                              [--required-only]\n",
    "                              [--variant {auto,cpu,cuda,coreml}]\n",
);

pub const INSTALL_MODELS_HELP: &str = concat!(
    "usage: journal install-models [-h] [--check | --force]\n",
    "                              [--required-only]\n",
    "                              [--variant {auto,cpu,cuda,coreml}]\n",
    "\n",
    "install and verify solstone's bundled ML models (local STT plus bundled\n",
    "wespeaker/pyannote assets). default action checks the local STT artifacts and\n",
    "fetches if missing; --force re-fetches; --check verifies only and exits\n",
    "nonzero on any problem.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --check               verify bundled assets and local STT artifacts without\n",
    "                        fetching.\n",
    "  --force               ignore readiness and refetch/verify local STT\n",
    "                        artifacts.\n",
    "  --required-only       install or verify only bundled models required for\n",
    "                        journal readiness; do not fetch optional providers.\n",
    "  --variant {auto,cpu,cuda,coreml}\n",
    "                        journal variant to install or verify. auto honors\n",
    "                        JOURNAL_VARIANT on linux/x86_64, then autodetects.\n",
);

pub const INSTALL_PROVIDER_USAGE: &str = "usage: journal install-provider [-h] name\n";

pub const INSTALL_PROVIDER_HELP: &str = concat!(
    "usage: journal install-provider [-h] name\n",
    "\n",
    "Install or retry a provider runtime.\n",
    "\n",
    "positional arguments:\n",
    "  name        Provider to install: 'local' or 'parakeet'.\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
);

pub const THINKING_USAGE: &str = "usage: journal thinking [-h] {set-lane} ...\n";

pub const THINKING_HELP: &str = concat!(
    "usage: journal thinking [-h] {set-lane} ...\n",
    "\n",
    "Select the journal thinking lane.\n",
    "\n",
    "positional arguments:\n",
    "  {set-lane}\n",
    "    set-lane             Set the thinking lane (local, byo, or confidential)\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
);

pub const THINKING_SET_LANE_USAGE: &str = concat!(
    "usage: journal thinking set-lane [-h] {local,byo,confidential} ",
    "[--provider PROVIDER] [--model MODEL] [--journal PATH]\n",
);

pub const THINKING_SET_LANE_HELP: &str = concat!(
    "usage: journal thinking set-lane [-h] {local,byo,confidential} ",
    "[--provider PROVIDER] [--model MODEL] [--journal PATH]\n",
    "\n",
    "Set the thinking lane used by generate and cogitate.\n",
    "\n",
    "positional arguments:\n",
    "  {local,byo,confidential}\n",
    "                        Thinking lane to activate.\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --provider PROVIDER   BYO provider: anthropic, google, local, or openai\n",
    "  --model MODEL         Cloud BYO model name\n",
    "  --journal PATH        Journal root\n",
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

/// `journal convey --help`, captured verbatim from the retained owner command.
pub const CONVEY_HELP: &str = concat!(
    "usage: journal convey [-h] --port PORT [-v] [-d]\n",
    "\n",
    "Convey web interface\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  --port PORT    Port to serve on\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

/// The parse-error usage for `journal convey`.
pub const CONVEY_USAGE: &str = "usage: journal convey [-h] --port PORT [-v] [-d]\n";

/// `journal schedule --help`, captured from the retained scheduler CLI.
pub const SCHEDULE_HELP: &str = concat!(
    "usage: journal schedule [-h] [-v] [-d]\n",
    "\n",
    "Show scheduled tasks\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

/// The parse-error usage for `journal schedule`.
pub const SCHEDULE_USAGE: &str = "usage: journal schedule [-h] [-v] [-d]\n";

/// The parse-error usage for `journal spl`.
pub const SPL_USAGE: &str = "usage: journal spl [-h] [-v] [-d]\n";

pub const SPL_HELP: &str = concat!(
    "usage: journal spl [-h] [-v] [-d]\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

/// The parse-error usage for `journal mcp`.
pub const MCP_USAGE: &str = "usage: journal mcp [-h] {service,local-door,status,token,pairing,oauth,permission,activity,probe} ...\n";

/// `journal mcp --help`.
pub const MCP_HELP: &str = concat!(
    "usage: journal mcp [-h] {service,local-door,status,token,pairing,oauth,permission,activity,probe} ...\n",
    "\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "\n",
    "subcommands:\n",
    "  service               Run the MCP endpoint\n",
    "  local-door            Run the direct local door\n",
    "  status                Show MCP endpoint status\n",
    "  token                 Manage MCP bearer tokens\n",
    "  pairing               Manage the agent pairing code\n",
    "  oauth                 Manage registered OAuth clients\n",
    "  permission            Manage connection read permissions\n",
    "  activity              Show what connections asked for and how each request ended\n",
    "  probe                 Invoke one MCP read without opening a port\n",
);

pub const BACKFILL_FACET_IDS_USAGE: &str = "usage: journal backfill-facet-ids [-h] [--commit]\n";

pub const BACKFILL_FACET_IDS_HELP: &str = concat!(
    "usage: journal backfill-facet-ids [-h] [--commit]\n",
    "\n",
    "Backfill missing UUIDv4 identifiers across all facet declarations.\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --commit    Write backfilled facet IDs to disk (default is dry run)\n",
);

pub const TALENT_USAGE: &str =
    "usage: journal talent [-h] [-v] [-d] {list,inventory,show,logs,log} ...\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Config(ConfigCommand),
    ConfigUsage,
    ConfigHelp,
    Doctor(solstone_core_doctor::args::DoctorArgs),
    DoctorUsage(solstone_core_doctor::args::DoctorUsageError),
    DoctorHelp,
    Setup(solstone_core_setup::args::SetupArgs),
    SetupUsage(solstone_core_setup::args::UsageError),
    SetupHelp,
    Version,
    Assets,
    Warm { json: bool },
    Check { json: bool },
    CheckUsage,
    CheckHelp,
    JournalPath(JournalPathOptions),
    Indexer(Box<IndexerCommand>),
    JournalConfig(JournalConfigCommand),
    SpeakerTranscriptWrite,
    SpeakerResolve(SpeakerResolveCommand),
    Local(LocalCommand),
    Generate(GenerateCommand),
    Cogitate(CogitateCommand),
    Brain(BrainCommand),
    JournalBrainOwner(JournalBrainOwnerCommand),
    Body(BodyCommand),
    Transcribe(TranscribeOptions),
    Think(Vec<OsString>),
    Thinking(ThinkingCommand),
    ThinkingUsage,
    ThinkingHelp,
    Streams(Vec<OsString>),
    Importer(Vec<OsString>),
    Segment(Vec<OsString>),
    Backup(Vec<OsString>),
    Maintenance(Vec<OsString>),
    TalentWorker(Vec<OsString>),
    ParentLossCoordinator(Vec<OsString>),
    JournalRouteInspect,
    JournalRouteRepair { lock_owner: String },
    Reprocess(Vec<OsString>),
    JournalStats(Vec<OsString>),
    Talent(Vec<OsString>),
    Backfill(Vec<OsString>),
    BackfillFacetIds { commit: bool },
    BackfillFacetIdsHelp,
    BackfillFacetIdsUsage,
    FacetCandidates,
    InstallModels(InstallModelsOptions),
    InstallModelsUsage,
    InstallModelsHelp,
    InstallProvider(InstallProviderOptions),
    InstallProviderUsage,
    InstallProviderHelp,
    Convey(ConveyOptions),
    ConveyHelp,
    ConveyUsage(ConveyUsageError),
    Schedule(ScheduleOptions),
    ScheduleHelp,
    ScheduleUsage(ScheduleUsageError),
    Grab(GrabCommand),
    Spl(SplCommand),
    SplUsage(SplUsageError),
    SplHelp,
    Mcp(McpCommand),
    McpUsage(McpUsageError),
    McpHelp,
    Sense(SenseOptions),
    SenseUsage,
    SenseHelp,
    Cortex(ServiceOptions),
    CortexUsage(CortexUsageError),
    CortexHelp,
    Supervisor(SupervisorOptions),
    SupervisorUsage,
    SupervisorInvalid(SupervisorUsageError),
    SupervisorHelp,
    StartUsage,
    StartInvalid(SupervisorUsageError),
    StartHelp,
    SupervisorLifecycleRedirect(&'static str),
    Health { verbose: bool, debug: bool },
    HealthUsage,
    HealthHelp,
    Top { verbose: bool, debug: bool },
    TopUsage,
    TopHelp,
    HealthLogs(HealthLogsArgs),
    HealthLogsUsage(HealthLogsArgs),
    HealthLogsHelp(HealthLogsArgs),
    Heartbeat { force: bool },
    HeartbeatUsage,
    HeartbeatHelp,
    Engage(EngageOptions),
    EngageUsage,
    EngageHelp,
    Service(ServiceParseOutcome),
    Navigate { path: String },
    NavigateUsage,
    NavigateFacetRetired(&'static str),
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
    Contract(ContractCommand),
    ContractUsage,
    ContractHelp,
    ContractBuildUsage,
    ContractBuildHelp,
    ContractCheckUsage,
    ContractCheckHelp,
    TranscribeHelp,
    FacetCandidatesHelp,
    FacetCandidatesUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalBrainOwnerCommand {
    Status {
        json: bool,
    },
    Refresh(JournalBrainRefreshOptions),
    RenewPrerequisites {
        json: bool,
        expected_fingerprint: Option<String>,
    },
    Help,
    StatusHelp,
    RefreshHelp,
    RenewPrerequisitesHelp,
    Bare,
    Usage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalBrainRefreshOptions {
    pub json: bool,
    pub expected_fingerprint: Option<String>,
    pub expected_active_fingerprint: bool,
    pub expect_active_fingerprint_absent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthLogsArgs {
    pub count: String,
    pub follow: bool,
    pub since: Option<String>,
    pub service: Option<String>,
    pub grep: Option<String>,
    pub verbose: bool,
    pub debug: bool,
    pub value_checks: Vec<HealthLogsValueCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthLogsValueCheck {
    Count(String),
    Since(String),
    Grep(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceLogsArgs {
    pub follow: bool,
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
    pub required_only: bool,
    pub variant: InstallModelsVariant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallProviderOptions {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingSetLaneOptions {
    pub lane: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingCommand {
    SetLane(ThinkingSetLaneOptions),
    SetLaneUsage,
    SetLaneHelp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityCommand {
    Hydrate,
    Partner(IdentityPartnerOptions),
    Health(IdentityHealthOptions),
    Briefing(IdentityBriefingOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractCommand {
    Build {
        check: bool,
        root: Option<PathBuf>,
    },
    Check {
        journals: Vec<PathBuf>,
        root: Option<PathBuf>,
    },
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
pub struct ConveyUsageError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CortexUsageError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleOptions {
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleUsageError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplUsageError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpUsageError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorOptions {
    pub port: u16,
    pub journal_override: Option<OsString>,
    pub no_daily: bool,
    pub no_schedule: bool,
    pub no_convey: bool,
    pub no_cortex: bool,
    pub no_spl: bool,
    pub direct_port: Option<u16>,
    pub hosted_parent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorUsageError(pub String);

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
    VerifySha256,
    CudaTrust,
    ManifestVulkan,
    ManifestCuda,
    ManifestModel,
    InspectLocal,
    InspectParakeet,
    ProbeBinary,
    RunLocal,
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
pub enum ConfigCommand {
    Show,
    Journal(ConfigJournalOptions),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAction {
    Move,
    Switch,
    Force,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigJournalOptions {
    pub path: String,
    pub action: Option<ConfigAction>,
    pub yes: bool,
    pub dry_run: bool,
}

pub const CONFIG_USAGE: &str = concat!(
    "usage: journal config [-h] {show,journal} ...\n",
    "       journal config journal PATH [--move | --switch | --force] [--yes | --dry-run]\n"
);
pub const CONFIG_HELP: &str = concat!(
    "usage: journal config [-h] {show,journal} ...\n",
    "       journal config journal PATH [--move | --switch | --force] [--yes | --dry-run]\n\n",
    "positional arguments:\n  {show,journal}\n",
    "    show          show the configured journal path and source\n",
    "    journal PATH  change the journal path used by this installation\n"
);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCommand {
    Service,
    LocalDoor,
    Status,
    Token(McpTokenCommand),
    Pairing(McpPairingCommand),
    Oauth(McpOauthCommand),
    Permission(McpPermissionCommand),
    Activity(McpActivityCommand),
    Probe(McpProbeCommand),
}

/// An owner's filter over recorded agent activity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpActivityCommand {
    pub target: Option<McpTarget>,
    pub tool: Option<String>,
    pub outcome: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub limit: usize,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTarget {
    Token {
        label: String,
    },
    Oauth {
        client_id: String,
        created_at: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpPermissionCommand {
    Show {
        target: Option<McpTarget>,
    },
    Set {
        target: McpTarget,
        categories: Vec<String>,
        facets: Vec<String>,
    },
    Clear {
        target: McpTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpProbeCommand {
    pub target: McpTarget,
    pub tool: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTokenCommand {
    Create { label: String },
    List,
    Revoke { label: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpPairingCommand {
    Generate { door: Option<String> },
    Revoke,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpOauthCommand {
    List,
    Revoke { client_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOptions {
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenseReprocessKind {
    Screen,
    Audio,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenseOptions {
    pub day: Option<String>,
    pub jobs: i64,
    pub reprocess: Option<SenseReprocessKind>,
    pub segment: Option<String>,
    pub stream: Option<String>,
    pub dry_run: bool,
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
        [sentinel, command, rest @ ..]
            if sentinel == OsStr::new(JOURNAL_BRAIN_OWNER_SENTINEL)
                && command == OsStr::new("brain") =>
        {
            Ok(Command::JournalBrainOwner(parse_journal_brain_owner(rest)))
        }
        [command, rest @ ..] if command == OsStr::new("doctor") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                Ok(Command::DoctorHelp)
            } else {
                Ok(solstone_core_doctor::args::parse_doctor_args(rest)
                    .map_or_else(Command::DoctorUsage, Command::Doctor))
            }
        }
        [command, rest @ ..] if command == OsStr::new("setup") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                Ok(Command::SetupHelp)
            } else {
                Ok(solstone_core_setup::args::parse_args(rest)
                    .map_or_else(Command::SetupUsage, Command::Setup))
            }
        }
        [flag] if flag == OsStr::new("--version") => Ok(Command::Version),
        [command] if command == OsStr::new("assets") => Ok(Command::Assets),
        [command] if command == OsStr::new("warm") => Ok(Command::Warm { json: false }),
        [command, flag] if command == OsStr::new("warm") && flag == OsStr::new("--json") => {
            Ok(Command::Warm { json: true })
        }
        [command, rest @ ..] if command == OsStr::new("check") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                Ok(Command::CheckHelp)
            } else {
                match rest {
                    [] => Ok(Command::Check { json: false }),
                    [flag] if flag == OsStr::new("--json") => Ok(Command::Check { json: true }),
                    _ => Ok(Command::CheckUsage),
                }
            }
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
        [command, rest @ ..] if command == OsStr::new("config") => {
            let help = |arg: &OsString| arg == OsStr::new("-h") || arg == OsStr::new("--help");
            if rest.iter().any(help) {
                Ok(Command::ConfigHelp)
            } else {
                Ok(parse_config(rest).map_or(Command::ConfigUsage, Command::Config))
            }
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
        [command, rest @ ..] if command == OsStr::new("contract") => parse_contract(rest),
        [command, rest @ ..] if command == OsStr::new("transcribe") => parse_transcribe(rest),
        [command, rest @ ..] if command == OsStr::new("think") => Ok(Command::Think(rest.to_vec())),
        [command, rest @ ..] if command == OsStr::new("thinking") => Ok(parse_thinking(rest)),
        [command, rest @ ..] if command == OsStr::new("streams") => {
            Ok(Command::Streams(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("importer") => {
            Ok(Command::Importer(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("segment") => {
            Ok(Command::Segment(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("backup") => {
            Ok(Command::Backup(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("maintenance") => {
            Ok(Command::Maintenance(rest.to_vec()))
        }
        [command, ..] if command == OsStr::new("__talent-worker") => {
            Ok(Command::TalentWorker(args.to_vec()))
        }
        [command, ..] if command == OsStr::new("__parent-loss-coordinator") => {
            Ok(Command::ParentLossCoordinator(args.to_vec()))
        }
        [command] if command == OsStr::new("__journal-route-inspect") => {
            Ok(Command::JournalRouteInspect)
        }
        [command, ..] if command == OsStr::new("__journal-route-inspect") => Err(UsageError),
        [command, flag, owner]
            if command == OsStr::new("__journal-route-repair")
                && flag == OsStr::new("--route-lock-owner")
                && owner.to_str().is_some_and(is_route_lock_owner_token) =>
        {
            Ok(Command::JournalRouteRepair {
                lock_owner: owner.to_string_lossy().into_owned(),
            })
        }
        [command, ..] if command == OsStr::new("__journal-route-repair") => Err(UsageError),
        [command, rest @ ..] if command == OsStr::new("reprocess") => {
            Ok(Command::Reprocess(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("journal-stats") => {
            Ok(Command::JournalStats(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("talent") => {
            Ok(Command::Talent(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("backfill-processing-records") => {
            Ok(Command::Backfill(rest.to_vec()))
        }
        [command, rest @ ..] if command == OsStr::new("backfill-facet-ids") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                return Ok(Command::BackfillFacetIdsHelp);
            }
            match parse_backfill_facet_ids(rest) {
                Ok(commit) => Ok(Command::BackfillFacetIds { commit }),
                Err(()) => Ok(Command::BackfillFacetIdsUsage),
            }
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
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                return Ok(Command::InstallModelsHelp);
            }
            Ok(parse_install_models(rest)
                .map_or(Command::InstallModelsUsage, Command::InstallModels))
        }
        [command, rest @ ..] if command == OsStr::new("install-provider") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                return Ok(Command::InstallProviderHelp);
            }
            Ok(parse_install_provider(rest)
                .map_or(Command::InstallProviderUsage, Command::InstallProvider))
        }
        [command, rest @ ..] if command == OsStr::new("convey") => match parse_convey(rest) {
            Ok(ConveyParse::Run(options)) => Ok(Command::Convey(options)),
            Ok(ConveyParse::Help) => Ok(Command::ConveyHelp),
            Err(error) => Ok(Command::ConveyUsage(error)),
        },
        [command, rest @ ..] if command == OsStr::new("schedule") => match parse_schedule(rest) {
            Ok(ScheduleParse::Run(options)) => Ok(Command::Schedule(options)),
            Ok(ScheduleParse::Help) => Ok(Command::ScheduleHelp),
            Err(error) => Ok(Command::ScheduleUsage(error)),
        },
        [command, rest @ ..] if command == OsStr::new("grab") => {
            Ok(Command::Grab(parse_grab(rest)))
        }
        [command, rest @ ..] if command == OsStr::new("spl") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                return Ok(Command::SplHelp);
            }
            match parse_spl(rest) {
                Ok(command) => Ok(Command::Spl(command)),
                Err(error) => Ok(Command::SplUsage(error)),
            }
        }
        [command, rest @ ..] if command == OsStr::new("mcp") => {
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                return Ok(Command::McpHelp);
            }
            match parse_mcp(rest) {
                Ok(command) => Ok(Command::Mcp(command)),
                Err(error) => Ok(Command::McpUsage(error)),
            }
        }
        [command, rest @ ..] if command == OsStr::new("sense") => match parse_sense(rest) {
            Ok(SenseParse::Run(options)) => Ok(Command::Sense(options)),
            Ok(SenseParse::Help) => Ok(Command::SenseHelp),
            Err(()) => Ok(Command::SenseUsage),
        },
        [command, rest @ ..] if command == OsStr::new("cortex") => match parse_cortex(rest) {
            Ok(CortexParse::Run(options)) => Ok(Command::Cortex(options)),
            Ok(CortexParse::Help) => Ok(Command::CortexHelp),
            Err(error) => Ok(Command::CortexUsage(error)),
        },
        [command, rest @ ..] if command == OsStr::new("supervisor") => Ok(
            parse_supervisor_invocation(rest, Command::SupervisorUsage, Command::SupervisorHelp),
        ),
        [command, rest @ ..] if command == OsStr::new("start") => Ok(parse_supervisor_invocation(
            rest,
            Command::StartUsage,
            Command::StartHelp,
        )),
        [command, rest @ ..] if command == OsStr::new("health") => {
            if let [first, logs @ ..] = rest
                && first == OsStr::new("logs")
            {
                return Ok(match parse_health_logs(logs) {
                    Ok(HealthLogsParse::Run(args)) => Command::HealthLogs(args),
                    Ok(HealthLogsParse::Help(args)) => Command::HealthLogsHelp(args),
                    Err(args) => Command::HealthLogsUsage(*args),
                });
            }
            let help = |argument: &OsString| {
                argument == OsStr::new("--help") || argument == OsStr::new("-h")
            };
            if rest.iter().any(help) {
                return Ok(Command::HealthHelp);
            }
            Ok(match parse_health(rest) {
                Ok((verbose, debug)) => Command::Health { verbose, debug },
                Err(()) => Command::HealthUsage,
            })
        }
        [command, rest @ ..] if command == OsStr::new("heartbeat") => {
            let option_end = rest
                .iter()
                .position(|argument| argument == OsStr::new("--"))
                .unwrap_or(rest.len());
            if rest[..option_end].iter().any(is_help) {
                return Ok(Command::HeartbeatHelp);
            }
            let trailing = if option_end == rest.len() {
                &rest[option_end..]
            } else {
                &rest[option_end + 1..]
            };
            Ok(
                if trailing.is_empty()
                    && rest[..option_end]
                        .iter()
                        .all(|argument| argument == OsStr::new("--force"))
                {
                    Command::Heartbeat {
                        force: option_end > 0,
                    }
                } else {
                    Command::HeartbeatUsage
                },
            )
        }
        [command, rest @ ..] if command == OsStr::new("engage") => Ok(match parse_engage(rest) {
            Ok(EngageParse::Run(options)) => Command::Engage(options),
            Ok(EngageParse::Help) => Command::EngageHelp,
            Err(UsageError) => Command::EngageUsage,
        }),
        [command, rest @ ..] if command == OsStr::new("top") => {
            if rest.iter().any(is_help) {
                return Ok(Command::TopHelp);
            }
            Ok(match parse_health(rest) {
                Ok((verbose, debug)) => Command::Top { verbose, debug },
                Err(()) => Command::TopUsage,
            })
        }
        [command, rest @ ..] if command == OsStr::new("service") => {
            Ok(Command::Service(parse_service_args(rest)))
        }
        [command, rest @ ..] if command == OsStr::new("up") => Ok(Command::Service(
            parse_up_down_alias(rest, ServiceAction::Up, UP_HELP),
        )),
        [command, rest @ ..] if command == OsStr::new("down") => Ok(Command::Service(
            parse_up_down_alias(rest, ServiceAction::Down, DOWN_HELP),
        )),
        _ => Err(UsageError),
    }
}

fn is_route_lock_owner_token(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

enum EngageParse {
    Run(EngageOptions),
    Help,
}

fn parse_engage(args: &[OsString]) -> Result<EngageParse, UsageError> {
    let mut wait = false;
    let mut facet = None;
    let mut day = None;
    let mut name = None;
    let mut index = 0;
    let mut positional_only = false;

    while index < args.len() {
        let argument = &args[index];
        if !positional_only && argument == OsStr::new("--") {
            positional_only = true;
            index += 1;
            continue;
        }
        if !positional_only && argument == OsStr::new("--wait") {
            wait = true;
            index += 1;
            continue;
        }
        if !positional_only && is_help(argument) {
            return Ok(EngageParse::Help);
        }
        if !positional_only
            && (argument == OsStr::new("--facet") || argument == OsStr::new("--day"))
        {
            let value = args
                .get(index + 1)
                .and_then(|value| value.to_str())
                .ok_or(UsageError)?;
            if argument == OsStr::new("--facet") {
                facet = Some(value.to_owned());
            } else {
                day = Some(value.to_owned());
            }
            index += 2;
            continue;
        }
        if !positional_only {
            let text = argument.to_str().ok_or(UsageError)?;
            if let Some(value) = text.strip_prefix("--facet=") {
                facet = Some(value.to_owned());
                index += 1;
                continue;
            }
            if let Some(value) = text.strip_prefix("--day=") {
                day = Some(value.to_owned());
                index += 1;
                continue;
            }
            if text.starts_with('-') {
                return Err(UsageError);
            }
        }
        let value = argument.to_str().ok_or(UsageError)?;
        if name.replace(value.to_owned()).is_some() {
            return Err(UsageError);
        }
        index += 1;
    }

    Ok(EngageParse::Run(EngageOptions {
        name: name.ok_or(UsageError)?,
        wait,
        facet,
        day,
    }))
}

fn parse_journal_brain_owner(args: &[OsString]) -> JournalBrainOwnerCommand {
    let mut start = 0;
    // argparse attaches these parent options before selecting a subparser.
    while matches!(args.get(start).map(OsString::as_os_str), Some(value) if value == OsStr::new("-v") || value == OsStr::new("--verbose") || value == OsStr::new("-d") || value == OsStr::new("--debug"))
    {
        start += 1;
    }
    let args = &args[start..];
    if args.is_empty() {
        return JournalBrainOwnerCommand::Bare;
    }
    if matches!(args.first(), Some(argument) if argument == OsStr::new("-h") || argument == OsStr::new("--help"))
    {
        return JournalBrainOwnerCommand::Help;
    }
    match args {
        [command, rest @ ..] if command == OsStr::new("status") => {
            if rest.iter().any(is_help) {
                return JournalBrainOwnerCommand::StatusHelp;
            }
            let mut json = false;
            for arg in rest {
                if arg == OsStr::new("--json") {
                    json = true;
                } else {
                    return JournalBrainOwnerCommand::Usage;
                }
            }
            JournalBrainOwnerCommand::Status { json }
        }
        [command, rest @ ..] if command == OsStr::new("refresh") => {
            if rest.iter().any(is_help) {
                return JournalBrainOwnerCommand::RefreshHelp;
            }
            let mut options = JournalBrainRefreshOptions {
                json: false,
                expected_fingerprint: None,
                expected_active_fingerprint: false,
                expect_active_fingerprint_absent: false,
            };
            let mut index = 0;
            while index < rest.len() {
                match rest[index].to_str() {
                    Some("--json") => options.json = true,
                    Some("--expected-active-fingerprint") => {
                        options.expected_active_fingerprint = true
                    }
                    Some("--expect-active-fingerprint-absent") => {
                        options.expect_active_fingerprint_absent = true
                    }
                    Some("--expected-fingerprint") => {
                        let Some(value) = rest.get(index + 1) else {
                            return JournalBrainOwnerCommand::Usage;
                        };
                        // argparse accepts arbitrary argv bytes as an option
                        // value; the writer later treats a non-SHA value as
                        // stale rather than turning it into a usage error.
                        options.expected_fingerprint = Some(value.to_string_lossy().into_owned());
                        index += 1;
                    }
                    _ => return JournalBrainOwnerCommand::Usage,
                }
                index += 1;
            }
            JournalBrainOwnerCommand::Refresh(options)
        }
        [command, rest @ ..] if command == OsStr::new("renew-prerequisites") => {
            if rest.iter().any(is_help) {
                return JournalBrainOwnerCommand::RenewPrerequisitesHelp;
            }
            let mut json = false;
            let mut expected_fingerprint = None;
            let mut index = 0;
            while index < rest.len() {
                match rest[index].to_str() {
                    Some("--json") => json = true,
                    Some("--expected-fingerprint") => {
                        let Some(value) = rest.get(index + 1) else {
                            return JournalBrainOwnerCommand::Usage;
                        };
                        expected_fingerprint = Some(value.to_string_lossy().into_owned());
                        index += 1;
                    }
                    _ => return JournalBrainOwnerCommand::Usage,
                };
                index += 1;
            }
            JournalBrainOwnerCommand::RenewPrerequisites {
                json,
                expected_fingerprint,
            }
        }
        _ => JournalBrainOwnerCommand::Usage,
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

fn parse_contract(args: &[OsString]) -> Result<Command, UsageError> {
    let Some((verb, rest)) = args.split_first() else {
        return Ok(Command::ContractUsage);
    };
    if verb == OsStr::new("--help") || verb == OsStr::new("-h") {
        return Ok(Command::ContractHelp);
    }
    match verb.to_str() {
        Some("build") => parse_contract_build(rest),
        Some("check") => parse_contract_check(rest),
        _ => Ok(Command::ContractUsage),
    }
}

fn parse_contract_build(args: &[OsString]) -> Result<Command, UsageError> {
    if args
        .iter()
        .any(|arg| arg == OsStr::new("--help") || arg == OsStr::new("-h"))
    {
        return Ok(Command::ContractBuildHelp);
    }
    let mut check = false;
    let mut root = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--check") {
            check = true;
            index += 1;
        } else if argument == OsStr::new("--root") {
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value.to_string_lossy().starts_with('-') || root.is_some() {
                return Ok(Command::ContractBuildUsage);
            }
            root = Some(PathBuf::from(value));
            index += 2;
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--root="))
        {
            if value.is_empty() || root.is_some() {
                return Ok(Command::ContractBuildUsage);
            }
            root = Some(PathBuf::from(value));
            index += 1;
        } else {
            return Ok(Command::ContractBuildUsage);
        }
    }
    Ok(Command::Contract(ContractCommand::Build { check, root }))
}

fn parse_contract_check(args: &[OsString]) -> Result<Command, UsageError> {
    if args
        .iter()
        .any(|arg| arg == OsStr::new("--help") || arg == OsStr::new("-h"))
    {
        return Ok(Command::ContractCheckHelp);
    }
    let mut journals = Vec::new();
    let mut root = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--journal") {
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value.to_string_lossy().starts_with('-') {
                return Ok(Command::ContractCheckUsage);
            }
            journals.push(PathBuf::from(value));
            index += 2;
        } else if argument == OsStr::new("--root") {
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value.to_string_lossy().starts_with('-') || root.is_some() {
                return Ok(Command::ContractCheckUsage);
            }
            root = Some(PathBuf::from(value));
            index += 2;
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--journal="))
        {
            if value.is_empty() {
                return Ok(Command::ContractCheckUsage);
            }
            journals.push(PathBuf::from(value));
            index += 1;
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--root="))
        {
            if value.is_empty() || root.is_some() {
                return Ok(Command::ContractCheckUsage);
            }
            root = Some(PathBuf::from(value));
            index += 1;
        } else {
            return Ok(Command::ContractCheckUsage);
        }
    }
    Ok(Command::Contract(ContractCommand::Check { journals, root }))
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
    let mut literal = false;
    let mut index = 0;

    while index < args.len() {
        let argument = &args[index];
        if !literal && argument == OsStr::new("--") {
            literal = true;
            index += 1;
            continue;
        }
        if !literal && let Some(option) = retired_navigate_facet_option(argument) {
            return Ok(Command::NavigateFacetRetired(option));
        }
        if !literal && argument.to_str().is_none_or(|value| value.starts_with('-')) {
            return Err(UsageError);
        }
        if path.is_some() {
            return Err(UsageError);
        }
        let value = argument.to_str().ok_or(UsageError)?;
        if value.is_empty() {
            return Err(UsageError);
        }
        path = Some(value.to_owned());
        index += 1;
    }

    path.map(|path| Command::Navigate { path })
        .ok_or(UsageError)
}

fn retired_navigate_facet_option(argument: &OsString) -> Option<&'static str> {
    let value = argument.to_str()?;
    if value == "--facet" || value.starts_with("--facet=") {
        Some("--facet")
    } else if value == "-f" || value.starts_with("-f") {
        Some("-f")
    } else {
        None
    }
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

fn parse_supervisor_invocation(rest: &[OsString], usage: Command, help: Command) -> Command {
    let is_start = matches!(usage, Command::StartUsage);
    if rest.iter().any(is_help) {
        return help;
    }
    match parse_supervisor(rest, is_start) {
        Ok(options) => Command::Supervisor(options),
        Err(SupervisorParseError::Invalid(error)) => {
            if is_start {
                Command::StartInvalid(error)
            } else {
                Command::SupervisorInvalid(error)
            }
        }
        Err(SupervisorParseError::Usage) => {
            supervisor_lifecycle_redirect(rest).map_or(usage, Command::SupervisorLifecycleRedirect)
        }
    }
}

fn parse_up_down_alias(
    rest: &[OsString],
    action: ServiceAction,
    help: &'static str,
) -> ServiceParseOutcome {
    if rest.iter().any(is_help) {
        return ServiceParseOutcome::Exit {
            code: 0,
            stdout: Some(help),
            stderr: None,
        };
    }
    if rest.is_empty() {
        ServiceParseOutcome::Dispatch(action)
    } else {
        ServiceParseOutcome::Exit {
            code: 1,
            stdout: None,
            stderr: Some(SafeServiceDiagnostic::unknown_subcommand(
                rest[0].as_os_str(),
            )),
        }
    }
}

fn supervisor_lifecycle_redirect(args: &[OsString]) -> Option<&'static str> {
    args.iter().find_map(|argument| match argument.as_os_str() {
        value if value == OsStr::new("start") => Some("start"),
        value if value == OsStr::new("stop") => Some("stop"),
        value if value == OsStr::new("restart") => Some("restart"),
        value if value == OsStr::new("status") => Some("status"),
        value if value == OsStr::new("install") => Some("install"),
        value if value == OsStr::new("uninstall") => Some("uninstall"),
        value if value == OsStr::new("logs") => Some("logs"),
        _ => None,
    })
}

enum SupervisorParseError {
    Usage,
    Invalid(SupervisorUsageError),
}

fn parse_direct_port_value(value: &str) -> Result<u16, SupervisorParseError> {
    let parsed = value.parse::<i32>().map_err(|_| {
        SupervisorParseError::Invalid(SupervisorUsageError(format!(
            "argument --direct-port: invalid int value: '{value}'"
        )))
    })?;
    if !(1..=65535).contains(&parsed) {
        return Err(SupervisorParseError::Invalid(SupervisorUsageError(
            "argument --direct-port: must be between 1 and 65535".to_owned(),
        )));
    }
    Ok(parsed as u16)
}

fn parse_supervisor(
    args: &[OsString],
    is_start: bool,
) -> Result<SupervisorOptions, SupervisorParseError> {
    let mut port = 0;
    let mut port_consumed = false;
    let mut journal_override = None;
    let mut no_daily = false;
    let mut no_schedule = false;
    let mut no_convey = false;
    let mut no_cortex = false;
    let mut no_spl = false;
    let mut direct_port = None;
    let mut hosted_parent = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_os_str() {
            value if value == OsStr::new("--no-daily") => {
                if no_daily {
                    return Err(SupervisorParseError::Usage);
                }
                no_daily = true;
                index += 1;
            }
            value if value == OsStr::new("--no-schedule") => {
                if no_schedule {
                    return Err(SupervisorParseError::Usage);
                }
                no_schedule = true;
                index += 1;
            }
            value if value == OsStr::new("--no-convey") => {
                if no_convey {
                    return Err(SupervisorParseError::Usage);
                }
                no_convey = true;
                index += 1;
            }
            value if value == OsStr::new("--no-cortex") => {
                if no_cortex {
                    return Err(SupervisorParseError::Usage);
                }
                no_cortex = true;
                index += 1;
            }
            value if value == OsStr::new("--no-spl") => {
                if no_spl {
                    return Err(SupervisorParseError::Usage);
                }
                no_spl = true;
                index += 1;
            }
            value if is_start && value == OsStr::new("--hosted-parent") => {
                if hosted_parent {
                    return Err(SupervisorParseError::Usage);
                }
                hosted_parent = true;
                index += 1;
            }
            value if value == OsStr::new("--direct-port") => {
                if direct_port.is_some() {
                    return Err(SupervisorParseError::Invalid(SupervisorUsageError(
                        "argument --direct-port: cannot be repeated".to_owned(),
                    )));
                }
                let value = args.get(index + 1).ok_or_else(|| {
                    SupervisorParseError::Invalid(SupervisorUsageError(
                        "argument --direct-port: expected one argument".to_owned(),
                    ))
                })?;
                let value = value.to_str().ok_or_else(|| {
                    SupervisorParseError::Invalid(SupervisorUsageError(
                        "argument --direct-port: invalid int value".to_owned(),
                    ))
                })?;
                direct_port = Some(parse_direct_port_value(value)?);
                index += 2;
            }
            value
                if value
                    .to_str()
                    .is_some_and(|item| item.starts_with("--direct-port=")) =>
            {
                if direct_port.is_some() {
                    return Err(SupervisorParseError::Invalid(SupervisorUsageError(
                        "argument --direct-port: cannot be repeated".to_owned(),
                    )));
                }
                let value = value
                    .to_str()
                    .and_then(|item| item.strip_prefix("--direct-port="))
                    .expect("prefix checked");
                direct_port = Some(parse_direct_port_value(value)?);
                index += 1;
            }
            value if value == OsStr::new("--journal") => {
                if journal_override.is_some() {
                    return Err(SupervisorParseError::Usage);
                }
                let value = args.get(index + 1).ok_or(SupervisorParseError::Usage)?;
                if value.to_string_lossy().starts_with("--") {
                    return Err(SupervisorParseError::Usage);
                }
                journal_override = Some(value.clone());
                index += 2;
            }
            value if !port_consumed => {
                port = value
                    .to_str()
                    .ok_or(SupervisorParseError::Usage)?
                    .parse()
                    .map_err(|_| SupervisorParseError::Usage)?;
                port_consumed = true;
                index += 1;
            }
            _ => return Err(SupervisorParseError::Usage),
        }
    }
    Ok(SupervisorOptions {
        port,
        journal_override,
        no_daily,
        no_schedule,
        no_convey,
        no_cortex,
        no_spl,
        direct_port,
        hosted_parent,
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

enum ConveyParse {
    Run(ConveyOptions),
    Help,
}

fn parse_convey(args: &[OsString]) -> Result<ConveyParse, ConveyUsageError> {
    let mut port = None;
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("-h") || argument == OsStr::new("--help") {
            return Ok(ConveyParse::Help);
        }
        if matches!(
            argument.to_str(),
            Some("-v" | "--verbose" | "-d" | "--debug")
        ) {
            index += 1;
            continue;
        }
        if argument == OsStr::new("--port") {
            let value = args.get(index + 1).ok_or_else(|| {
                ConveyUsageError("argument --port: expected one argument".to_owned())
            })?;
            let value = value
                .to_str()
                .ok_or_else(|| ConveyUsageError("argument --port: invalid int value".to_owned()))?;
            let parsed = value.parse::<i32>().map_err(|_| {
                ConveyUsageError(format!("argument --port: invalid int value: '{value}'"))
            })?;
            if !(1..=65535).contains(&parsed) {
                return Err(ConveyUsageError(
                    "argument --port: must be between 1 and 65535".to_owned(),
                ));
            }
            port = Some(parsed as u16);
            index += 2;
            continue;
        }
        if let Some(value) = argument
            .to_str()
            .and_then(|item| item.strip_prefix("--port="))
        {
            let parsed = value.parse::<i32>().map_err(|_| {
                ConveyUsageError(format!("argument --port: invalid int value: '{value}'"))
            })?;
            if !(1..=65535).contains(&parsed) {
                return Err(ConveyUsageError(
                    "argument --port: must be between 1 and 65535".to_owned(),
                ));
            }
            port = Some(parsed as u16);
            index += 1;
            continue;
        }
        if argument == OsStr::new("--journal") {
            let value = args.get(index + 1).ok_or_else(|| {
                ConveyUsageError("argument --journal: expected one argument".to_owned())
            })?;
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(ConveyUsageError(format!(
            "unrecognized arguments: {}",
            args[index..]
                .iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        )));
    }
    Ok(ConveyParse::Run(ConveyOptions {
        port: port.ok_or_else(|| {
            ConveyUsageError("the following arguments are required: --port".to_owned())
        })?,
        journal_override,
    }))
}

enum ScheduleParse {
    Run(ScheduleOptions),
    Help,
}

fn parse_schedule(args: &[OsString]) -> Result<ScheduleParse, ScheduleUsageError> {
    let mut verbose = false;
    let mut debug = false;
    for (index, argument) in args.iter().enumerate() {
        match argument.to_str() {
            Some("-h" | "--help") => return Ok(ScheduleParse::Help),
            Some("-v" | "--verbose") => verbose = true,
            Some("-d" | "--debug") => debug = true,
            _ => {
                return Err(ScheduleUsageError(format!(
                    "unrecognized arguments: {}",
                    args[index..]
                        .iter()
                        .map(|value| value.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ")
                )));
            }
        }
    }
    Ok(ScheduleParse::Run(ScheduleOptions { verbose, debug }))
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
    let mut required_only = false;
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
        } else if argument == OsStr::new("--required-only") {
            if required_only {
                return Err(UsageError);
            }
            required_only = true;
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
        required_only,
        variant,
    })
}

fn parse_install_provider(args: &[OsString]) -> Result<InstallProviderOptions, UsageError> {
    let [name] = args else {
        return Err(UsageError);
    };
    if name.as_os_str().as_encoded_bytes().starts_with(b"-") {
        return Err(UsageError);
    }
    Ok(InstallProviderOptions {
        name: name.to_str().ok_or(UsageError)?.to_owned(),
    })
}

fn parse_thinking(args: &[OsString]) -> Command {
    let Some((first, rest)) = args.split_first() else {
        return Command::ThinkingUsage;
    };
    if first == OsStr::new("--help") || first == OsStr::new("-h") {
        return Command::ThinkingHelp;
    }
    if first == OsStr::new("set-lane") {
        return Command::Thinking(parse_thinking_set_lane(rest));
    }
    Command::ThinkingUsage
}

fn parse_thinking_set_lane(args: &[OsString]) -> ThinkingCommand {
    let help =
        |argument: &OsString| argument == OsStr::new("--help") || argument == OsStr::new("-h");
    if args.iter().any(help) {
        return ThinkingCommand::SetLaneHelp;
    }
    let Some((lane_token, rest)) = args.split_first() else {
        return ThinkingCommand::SetLaneUsage;
    };
    if lane_token.as_os_str().as_encoded_bytes().starts_with(b"-") {
        return ThinkingCommand::SetLaneUsage;
    }
    let Some(lane) = lane_token.to_str() else {
        return ThinkingCommand::SetLaneUsage;
    };
    let mut provider = None;
    let mut model = None;
    let mut journal_override = None;
    let mut index = 0;
    while index < rest.len() {
        let argument = rest[index].as_os_str();
        if argument == OsStr::new("--provider")
            || argument == OsStr::new("--model")
            || argument == OsStr::new("--journal")
        {
            let Some(value) = rest.get(index + 1) else {
                return ThinkingCommand::SetLaneUsage;
            };
            if value.to_str().is_some_and(|value| value.starts_with("--")) {
                return ThinkingCommand::SetLaneUsage;
            }
            if argument == OsStr::new("--provider") {
                let Some(text) = value.to_str() else {
                    return ThinkingCommand::SetLaneUsage;
                };
                if provider.replace(text.to_owned()).is_some() {
                    return ThinkingCommand::SetLaneUsage;
                }
            } else if argument == OsStr::new("--model") {
                let Some(text) = value.to_str() else {
                    return ThinkingCommand::SetLaneUsage;
                };
                if model.replace(text.to_owned()).is_some() {
                    return ThinkingCommand::SetLaneUsage;
                }
            } else if journal_override.replace(value.clone()).is_some() {
                return ThinkingCommand::SetLaneUsage;
            }
            index += 2;
        } else {
            return ThinkingCommand::SetLaneUsage;
        }
    }
    ThinkingCommand::SetLane(ThinkingSetLaneOptions {
        lane: lane.to_owned(),
        provider,
        model,
        journal_override,
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
        [one, two] if one == OsStr::new("inspect") && two == OsStr::new("parakeet") => {
            Ok(InstallCommand::InspectParakeet)
        }
        [one] if one == OsStr::new("probe-binary") => Ok(InstallCommand::ProbeBinary),
        [one, two] if one == OsStr::new("run") && two == OsStr::new("local") => {
            Ok(InstallCommand::RunLocal)
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

fn parse_config(args: &[OsString]) -> Result<ConfigCommand, UsageError> {
    match args {
        [show] if show == OsStr::new("show") => Ok(ConfigCommand::Show),
        [journal, path, rest @ ..]
            if journal == OsStr::new("journal")
                && path.to_str().is_some_and(|path| !path.starts_with('-')) =>
        {
            let path = path.to_str().ok_or(UsageError)?.to_owned();
            let mut action = None;
            let mut yes = false;
            let mut dry_run = false;
            for arg in rest {
                match arg.to_str() {
                    Some("--move") if action.is_none() => action = Some(ConfigAction::Move),
                    Some("--switch") if action.is_none() => action = Some(ConfigAction::Switch),
                    Some("--force") if action.is_none() => action = Some(ConfigAction::Force),
                    Some("--yes") if !dry_run => yes = true,
                    Some("--dry-run") if !yes => dry_run = true,
                    _ => return Err(UsageError),
                }
            }
            Ok(ConfigCommand::Journal(ConfigJournalOptions {
                path,
                action,
                yes,
                dry_run,
            }))
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

fn parse_spl(args: &[OsString]) -> Result<SplCommand, SplUsageError> {
    let [command, rest @ ..] = args else {
        return Err(SplUsageError(
            "the following arguments are required: service".to_owned(),
        ));
    };
    if command != OsStr::new("service") {
        return Err(SplUsageError(format!(
            "invalid choice: {} (choose from service)",
            command.to_string_lossy()
        )));
    }
    parse_service(rest).map(SplCommand::Service).map_err(|_| {
        SplUsageError(format!(
            "unrecognized arguments: {}",
            rest.iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        ))
    })
}

fn parse_mcp(args: &[OsString]) -> Result<McpCommand, McpUsageError> {
    let [command, rest @ ..] = args else {
        return Err(McpUsageError(
            "the following arguments are required: service, local-door, status, token, pairing, oauth, permission, activity, probe"
                .to_owned(),
        ));
    };
    match command.to_str() {
        Some("service") if rest.is_empty() => Ok(McpCommand::Service),
        Some("local-door") if rest.is_empty() => Ok(McpCommand::LocalDoor),
        Some("status") if rest.is_empty() => Ok(McpCommand::Status),
        Some("token") => parse_mcp_token(rest).map(McpCommand::Token),
        Some("pairing") => parse_mcp_pairing(rest).map(McpCommand::Pairing),
        Some("oauth") => parse_mcp_oauth(rest).map(McpCommand::Oauth),
        Some("permission") => parse_mcp_permission(rest).map(McpCommand::Permission),
        Some("activity") => parse_mcp_activity(rest).map(McpCommand::Activity),
        Some("probe") => parse_mcp_probe(rest).map(McpCommand::Probe),
        _ => Err(McpUsageError(format!(
            "invalid MCP command: {}",
            command.to_string_lossy()
        ))),
    }
}

fn parse_mcp_permission(args: &[OsString]) -> Result<McpPermissionCommand, McpUsageError> {
    let [subcommand, rest @ ..] = args else {
        return Err(McpUsageError(
            "the following arguments are required: show, set, clear".to_owned(),
        ));
    };
    match subcommand.to_str() {
        Some("show") => {
            if rest.is_empty() {
                Ok(McpPermissionCommand::Show { target: None })
            } else {
                let target = parse_mcp_target(rest)?;
                Ok(McpPermissionCommand::Show {
                    target: Some(target),
                })
            }
        }
        Some("set") => parse_mcp_permission_set(rest),
        Some("clear") => {
            let target = parse_mcp_target(rest)?;
            Ok(McpPermissionCommand::Clear { target })
        }
        _ => Err(McpUsageError(format!(
            "invalid MCP permission command: {}",
            subcommand.to_string_lossy()
        ))),
    }
}

fn parse_mcp_permission_set(args: &[OsString]) -> Result<McpPermissionCommand, McpUsageError> {
    let mut target_args = Vec::new();
    let mut categories = Vec::new();
    let mut facets = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == OsStr::new("--category") {
            let value = iter
                .next()
                .ok_or_else(|| McpUsageError("expected argument after --category".to_owned()))?;
            let value = value
                .to_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    McpUsageError("category must be valid UTF-8 and nonempty".to_owned())
                })?;
            categories.push(value.to_owned());
        } else if let Some(value) = arg
            .to_str()
            .and_then(|value| value.strip_prefix("--category="))
        {
            if value.is_empty() {
                return Err(McpUsageError("category must be nonempty".to_owned()));
            }
            categories.push(value.to_owned());
        } else if arg == OsStr::new("--facet") {
            let value = iter
                .next()
                .ok_or_else(|| McpUsageError("expected argument after --facet".to_owned()))?;
            let value = value
                .to_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    McpUsageError("facet name must be valid UTF-8 and nonempty".to_owned())
                })?;
            facets.push(value.to_owned());
        } else if let Some(value) = arg
            .to_str()
            .and_then(|value| value.strip_prefix("--facet="))
        {
            if value.is_empty() {
                return Err(McpUsageError("facet name must be nonempty".to_owned()));
            }
            facets.push(value.to_owned());
        } else {
            target_args.push(arg.clone());
        }
    }
    let target = parse_mcp_target(&target_args)?;
    if categories.is_empty() {
        return Err(McpUsageError(
            "at least one --category is required".to_owned(),
        ));
    }
    Ok(McpPermissionCommand::Set {
        target,
        categories,
        facets,
    })
}

const DEFAULT_ACTIVITY_LIMIT: usize = 20;

fn parse_mcp_activity(args: &[OsString]) -> Result<McpActivityCommand, McpUsageError> {
    let mut target_args = Vec::new();
    let mut command = McpActivityCommand {
        limit: DEFAULT_ACTIVITY_LIMIT,
        ..McpActivityCommand::default()
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let (flag, inline) = match arg.to_str() {
            Some(value) => match value.split_once('=') {
                Some((flag, inline)) => (flag.to_owned(), Some(inline.to_owned())),
                None => (value.to_owned(), None),
            },
            None => {
                target_args.push(arg.clone());
                continue;
            }
        };
        let mut take = |name: &str| -> Result<String, McpUsageError> {
            match inline.clone() {
                Some(value) if !value.is_empty() => Ok(value),
                Some(_) => Err(McpUsageError(format!("{name} must be nonempty"))),
                None => iter
                    .next()
                    .and_then(|value| value.to_str().map(str::to_owned))
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| McpUsageError(format!("expected argument after {name}"))),
            }
        };
        match flag.as_str() {
            "--tool" => command.tool = Some(take("--tool")?),
            "--outcome" => command.outcome = Some(take("--outcome")?),
            "--day-from" => command.day_from = Some(validate_day(&take("--day-from")?)?),
            "--day-to" => command.day_to = Some(validate_day(&take("--day-to")?)?),
            "--limit" => {
                let value = take("--limit")?;
                command.limit = value
                    .parse::<usize>()
                    .ok()
                    .filter(|limit| (1..=1000).contains(limit))
                    .ok_or_else(|| {
                        McpUsageError("--limit must be between 1 and 1000".to_owned())
                    })?;
            }
            "--json" => command.json = true,
            // The connection-target flags belong to `parse_mcp_target`, which
            // reads them from the leftovers. Both the `--flag value` and the
            // `--flag=value` forms pass through untouched.
            "--token" | "--oauth" | "--created" => target_args.push(arg.clone()),
            // ⚠ Every other unrecognized flag stops here rather than falling
            // through to the target: `--tools search` would otherwise be
            // reported as a bad connection target rather than as the typo it is.
            other if other.starts_with("--") => {
                return Err(McpUsageError(format!(
                    "unknown option {other}; accepted: --token, --oauth, --created, --tool, \
                     --outcome, --day-from, --day-to, --limit, --json"
                )));
            }
            _ => target_args.push(arg.clone()),
        }
    }
    if !target_args.is_empty() {
        command.target = Some(parse_mcp_target(&target_args)?);
    }
    Ok(command)
}

/// ⚠ A day bound is compared as bytes against an eight-digit chronicle
/// directory name, so anything else silently compares wrong rather than
/// failing. Reject it here instead.
fn validate_day(value: &str) -> Result<String, McpUsageError> {
    if value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(value.to_owned())
    } else {
        Err(McpUsageError(format!(
            "day must be eight digits (YYYYMMDD), got {value:?}"
        )))
    }
}

fn parse_mcp_probe(args: &[OsString]) -> Result<McpProbeCommand, McpUsageError> {
    let mut target_args = Vec::new();
    let mut tool = None;
    let mut arguments = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == OsStr::new("--tool") {
            tool = Some(
                iter.next()
                    .ok_or_else(|| McpUsageError("expected argument after --tool".to_owned()))?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else if let Some(value) = arg.to_str().and_then(|value| value.strip_prefix("--tool=")) {
            tool = Some(value.to_owned());
        } else if arg == OsStr::new("--arguments") {
            arguments = Some(
                iter.next()
                    .ok_or_else(|| McpUsageError("expected argument after --arguments".to_owned()))?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else if let Some(value) = arg
            .to_str()
            .and_then(|value| value.strip_prefix("--arguments="))
        {
            arguments = Some(value.to_owned());
        } else {
            target_args.push(arg.clone());
        }
    }
    let tool = tool
        .filter(|value| !value.is_empty())
        .ok_or_else(|| McpUsageError("--tool is required".to_owned()))?;
    let arguments = arguments
        .filter(|value| !value.is_empty())
        .ok_or_else(|| McpUsageError("--arguments is required".to_owned()))?;
    Ok(McpProbeCommand {
        target: parse_mcp_target(&target_args)?,
        tool,
        arguments,
    })
}

fn parse_mcp_target(args: &[OsString]) -> Result<McpTarget, McpUsageError> {
    let mut token_label: Option<String> = None;
    let mut oauth_client_id: Option<String> = None;
    let mut oauth_created: Option<String> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == OsStr::new("--token") {
            let val = iter
                .next()
                .ok_or_else(|| McpUsageError("expected argument after --token".to_owned()))?;
            let s = val.to_str().filter(|s| !s.is_empty()).ok_or_else(|| {
                McpUsageError("token label must be valid UTF-8 and nonempty".to_owned())
            })?;
            token_label = Some(s.to_owned());
        } else if let Some(val) = arg.to_str().and_then(|s| s.strip_prefix("--token=")) {
            if val.is_empty() {
                return Err(McpUsageError("token label must be nonempty".to_owned()));
            }
            token_label = Some(val.to_owned());
        } else if arg == OsStr::new("--oauth") {
            let val = iter
                .next()
                .ok_or_else(|| McpUsageError("expected argument after --oauth".to_owned()))?;
            let s = val.to_str().filter(|s| !s.is_empty()).ok_or_else(|| {
                McpUsageError("oauth client-id must be valid UTF-8 and nonempty".to_owned())
            })?;
            oauth_client_id = Some(s.to_owned());
        } else if let Some(val) = arg.to_str().and_then(|s| s.strip_prefix("--oauth=")) {
            if val.is_empty() {
                return Err(McpUsageError("oauth client-id must be nonempty".to_owned()));
            }
            oauth_client_id = Some(val.to_owned());
        } else if arg == OsStr::new("--created") {
            let val = iter
                .next()
                .ok_or_else(|| McpUsageError("expected argument after --created".to_owned()))?;
            let s = val.to_str().filter(|s| !s.is_empty()).ok_or_else(|| {
                McpUsageError("created timestamp must be valid UTF-8 and nonempty".to_owned())
            })?;
            oauth_created = Some(s.to_owned());
        } else if let Some(val) = arg.to_str().and_then(|s| s.strip_prefix("--created=")) {
            if val.is_empty() {
                return Err(McpUsageError(
                    "created timestamp must be nonempty".to_owned(),
                ));
            }
            oauth_created = Some(val.to_owned());
        } else {
            return Err(McpUsageError(format!(
                "unexpected argument: {}",
                arg.to_string_lossy()
            )));
        }
    }

    match (token_label, oauth_client_id) {
        (Some(label), None) => {
            if oauth_created.is_some() {
                return Err(McpUsageError(
                    "--created cannot be used with --token".to_owned(),
                ));
            }
            Ok(McpTarget::Token { label })
        }
        (None, Some(client_id)) => Ok(McpTarget::Oauth {
            client_id,
            created_at: oauth_created,
        }),
        (Some(_), Some(_)) => Err(McpUsageError(
            "specify either --token or --oauth, not both".to_owned(),
        )),
        (None, None) => Err(McpUsageError(
            "one of --token or --oauth is required".to_owned(),
        )),
    }
}

fn parse_backfill_facet_ids(args: &[OsString]) -> Result<bool, ()> {
    match args {
        [] => Ok(false),
        [flag] if flag == OsStr::new("--commit") => Ok(true),
        _ => Err(()),
    }
}

fn parse_mcp_token(args: &[OsString]) -> Result<McpTokenCommand, McpUsageError> {
    let [command, rest @ ..] = args else {
        return Err(McpUsageError(
            "the following arguments are required: create, list, revoke".to_owned(),
        ));
    };
    match command.to_str() {
        Some("list") if rest.is_empty() => Ok(McpTokenCommand::List),
        Some("create") => {
            parse_mcp_token_label(rest).map(|label| McpTokenCommand::Create { label })
        }
        Some("revoke") => {
            parse_mcp_token_label(rest).map(|label| McpTokenCommand::Revoke { label })
        }
        _ => Err(McpUsageError(format!(
            "invalid MCP token command: {}",
            command.to_string_lossy()
        ))),
    }
}

fn parse_mcp_token_label(args: &[OsString]) -> Result<String, McpUsageError> {
    let [flag, label] = args else {
        return Err(McpUsageError(
            "the following arguments are required: --label LABEL".to_owned(),
        ));
    };
    if flag != OsStr::new("--label") || label.is_empty() {
        return Err(McpUsageError(
            "expected exactly one --label LABEL argument".to_owned(),
        ));
    }
    label
        .to_str()
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| McpUsageError("label must be valid UTF-8 and nonempty".to_owned()))
}

fn parse_mcp_pairing(args: &[OsString]) -> Result<McpPairingCommand, McpUsageError> {
    let [command, rest @ ..] = args else {
        return Err(McpUsageError(
            "the following arguments are required: generate, revoke".to_owned(),
        ));
    };
    match command.to_str() {
        Some("generate") => {
            if rest.is_empty() {
                Ok(McpPairingCommand::Generate { door: None })
            } else if let [flag, door] = rest {
                if flag != OsStr::new("--door") || door != OsStr::new("lan") {
                    return Err(McpUsageError(
                        "expected optional `--door lan` argument".to_owned(),
                    ));
                }
                Ok(McpPairingCommand::Generate {
                    door: Some("lan".to_owned()),
                })
            } else {
                Err(McpUsageError(
                    "expected optional `--door lan` argument".to_owned(),
                ))
            }
        }
        Some("revoke") if rest.is_empty() => Ok(McpPairingCommand::Revoke),
        _ => Err(McpUsageError(format!(
            "invalid MCP pairing command: {}",
            command.to_string_lossy()
        ))),
    }
}

fn parse_mcp_oauth(args: &[OsString]) -> Result<McpOauthCommand, McpUsageError> {
    let [command, rest @ ..] = args else {
        return Err(McpUsageError(
            "the following arguments are required: list, revoke".to_owned(),
        ));
    };
    match command.to_str() {
        Some("list") if rest.is_empty() => Ok(McpOauthCommand::List),
        Some("revoke") => {
            parse_mcp_oauth_client_id(rest).map(|client_id| McpOauthCommand::Revoke { client_id })
        }
        _ => Err(McpUsageError(format!(
            "invalid MCP oauth command: {}",
            command.to_string_lossy()
        ))),
    }
}

fn parse_mcp_oauth_client_id(args: &[OsString]) -> Result<String, McpUsageError> {
    let [flag, client_id] = args else {
        return Err(McpUsageError(
            "the following arguments are required: --client-id CLIENT_ID".to_owned(),
        ));
    };
    if flag != OsStr::new("--client-id") || client_id.is_empty() {
        return Err(McpUsageError(
            "expected exactly one --client-id CLIENT_ID argument".to_owned(),
        ));
    }
    client_id
        .to_str()
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| McpUsageError("client-id must be valid UTF-8 and nonempty".to_owned()))
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

enum SenseParse {
    Run(SenseOptions),
    Help,
}

enum CortexParse {
    Run(ServiceOptions),
    Help,
}

fn parse_cortex(args: &[OsString]) -> Result<CortexParse, CortexUsageError> {
    let mut verbose = false;
    let mut debug = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("-h" | "--help") => return Ok(CortexParse::Help),
            Some("-v" | "--verbose") if !verbose => verbose = true,
            Some("-d" | "--debug") if !debug => debug = true,
            _ => {
                return Err(CortexUsageError(format!(
                    "unrecognized arguments: {}",
                    args[index..]
                        .iter()
                        .map(|value| value.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ")
                )));
            }
        }
        index += 1;
    }
    Ok(CortexParse::Run(ServiceOptions { verbose, debug }))
}

fn parse_sense(args: &[OsString]) -> Result<SenseParse, ()> {
    let mut day = None;
    let mut jobs = 1;
    let mut reprocess = None;
    let mut segment = None;
    let mut stream = None;
    let mut dry_run = false;
    let mut verbose = false;
    let mut debug = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if matches!(argument.to_str(), Some("-h" | "--help")) {
            return Ok(SenseParse::Help);
        }
        match argument.to_str() {
            Some("-v" | "--verbose") => verbose = true,
            Some("-d" | "--debug") => debug = true,
            Some("--dry-run") => dry_run = true,
            Some("-j" | "--jobs") => {
                let value = sense_value(args, &mut index)?;
                jobs = value.parse::<i64>().map_err(|_| ())?;
            }
            Some("--day") => day = Some(sense_value(args, &mut index)?),
            Some("--segment") => segment = Some(sense_value(args, &mut index)?),
            Some("--stream") => stream = Some(sense_value(args, &mut index)?),
            Some("--reprocess") => {
                reprocess = Some(match sense_value(args, &mut index)?.as_str() {
                    "screen" => SenseReprocessKind::Screen,
                    "audio" => SenseReprocessKind::Audio,
                    "all" => SenseReprocessKind::All,
                    _ => return Err(()),
                });
            }
            Some(value) if value.starts_with("--day=") => {
                day = Some(value[6..].to_owned());
            }
            Some(value) if value.starts_with("--segment=") => {
                segment = Some(value[10..].to_owned());
            }
            Some(value) if value.starts_with("--stream=") => {
                stream = Some(value[9..].to_owned());
            }
            Some(value) if value.starts_with("--jobs=") => {
                jobs = value[7..].parse::<i64>().map_err(|_| ())?;
            }
            Some(value) if value.starts_with("--reprocess=") => {
                reprocess = Some(match &value[12..] {
                    "screen" => SenseReprocessKind::Screen,
                    "audio" => SenseReprocessKind::Audio,
                    "all" => SenseReprocessKind::All,
                    _ => return Err(()),
                });
            }
            _ => return Err(()),
        }
        index += 1;
    }
    // Match the reference's combination validation order before validating the
    // individual filter formats. This parser runs before journal resolution.
    if reprocess.is_some() && day.is_none()
        || segment.is_some() && day.is_none()
        || stream.is_some() && day.is_none()
        || dry_run && day.is_none()
    {
        return Err(());
    }
    if segment
        .as_deref()
        .is_some_and(|value| segment_key(value).is_none())
    {
        return Err(());
    }
    if stream
        .as_deref()
        .is_some_and(|value| !valid_stream_name(value))
    {
        return Err(());
    }
    Ok(SenseParse::Run(SenseOptions {
        day,
        jobs,
        reprocess,
        segment,
        stream,
        dry_run,
        verbose,
        debug,
    }))
}

fn sense_value(args: &[OsString], index: &mut usize) -> Result<String, ()> {
    *index += 1;
    let value = args.get(*index).ok_or(())?;
    if value.to_string_lossy().starts_with('-') {
        return Err(());
    }
    value.clone().into_string().map_err(|_| ())
}

fn valid_stream_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(byte) if byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn parse_health(args: &[OsString]) -> Result<(bool, bool), ()> {
    let mut verbose = false;
    let mut debug = false;
    for argument in args {
        let argument = argument.as_os_str();
        if argument == OsStr::new("-v") || argument == OsStr::new("--verbose") {
            if verbose {
                return Err(());
            }
            verbose = true;
            continue;
        }
        if argument == OsStr::new("-d") || argument == OsStr::new("--debug") {
            if debug {
                return Err(());
            }
            debug = true;
            continue;
        }
        return Err(());
    }
    Ok((verbose, debug))
}

fn parse_service_logs(args: &[OsString]) -> ServiceLogsArgs {
    ServiceLogsArgs {
        follow: args
            .iter()
            .any(|argument| argument == OsStr::new("-f") || argument == OsStr::new("--follow")),
    }
}

// SERVICE_ARGS_FOUNDATION_BEGIN
/// The usage text retained by the private service lifecycle grammar.
#[doc(hidden)]
pub const SERVICE_USAGE: &str = concat!(
    "Usage: journal service <install|uninstall|start|stop|restart|status|logs>\n",
    "       journal service install [--port PORT]  (default: 5015)\n",
    "       journal service restart [--if-installed]  (restart; --if-installed noops if not installed)\n",
    "       journal up                             (start + status; service must be installed)\n",
    "       journal down                           (stop)\n",
);

/// The hidden service verb the Windows post-update hook runs.
#[doc(hidden)]
pub const SERVICE_RESUME_AFTER_UPDATE: &str = "__resume-after-update";
/// The hidden service verb run inside Velopack's before-uninstall hook.
#[doc(hidden)]
pub const SERVICE_BEFORE_UNINSTALL: &str = "__before-uninstall";

/// A parsed service lifecycle action. This pure grammar is not executable.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceAction {
    Install {
        /// `None` when the owner named no `--port`. The platform then keeps
        /// the port the registration already carries, because re-registering
        /// an established journal onto the 5015 default silently moves it off
        /// its own port -- measured on Windows, true everywhere.
        port: Option<solstone_core_operational_logs::ServicePort>,
        installation_guard: Option<ServiceInstallationGuardArguments>,
    },
    Uninstall,
    Start,
    Stop,
    Restart {
        if_installed: bool,
    },
    Status,
    Logs {
        follow: bool,
    },
    Up,
    Down,
    /// Hidden: started by the Windows build's post-update and post-install
    /// hook to put back the resident an update took down. Never in usage text.
    ResumeAfterUpdate,
    /// Hidden: remove the task before Velopack sweeps its executable.
    BeforeUninstall,
}

/// The hidden identity fields carried from `journal setup` to service rendering.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInstallationGuardArguments {
    pub namespace: String,
    pub id: String,
    pub generation: String,
    pub journal_token: String,
}

/// A dynamic service diagnostic sanitized for exactly one terminal rendering.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeServiceDiagnostic(String);

/// Whether an argument is shaped like a port number, on the same axis the port
/// grammar reads: `parse_integer_text` strips one leading `+` or `-` before it
/// looks at digits, so the refusal has to as well. Underscores and non-ASCII
/// digits are deliberately out: the grammar takes them, but nobody types them
/// at a port and treating them as number-shaped would widen the range sentence
/// to arguments it does not describe.
fn is_port_shaped(text: &str) -> bool {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

impl SafeServiceDiagnostic {
    fn invalid_port_text(value: &str) -> Self {
        Self(format!(
            "error: invalid port '{}'",
            solstone_core_system_health::sanitize_for_terminal(value)
        ))
    }

    fn invalid_port_value(value: &OsStr) -> Self {
        Self(format!(
            "error: invalid port '{}'",
            solstone_core_system_health::sanitize_os_bytes_for_terminal(value.as_encoded_bytes())
        ))
    }

    fn incomplete_installation_guard() -> Self {
        // Left capitalized on purpose, unlike every owner-reachable message in
        // this type. This branch is only reachable with an `--installation-*`
        // flag, which is the hidden field set `journal setup` passes; it is an
        // assertion about a programming error, not owner copy. The owner-facing
        // form of this failure is the lowercase
        // "this installation couldn't be verified." recovery.
        Self("Error: installation identity arguments must be supplied together".to_owned())
    }

    fn unexpected_install_argument(value: &OsStr) -> Self {
        // A bare port was accepted silently and then ignored, so
        // `journal service install 6123` re-registered the task on the default
        // and exited 0. Name the argument, then add a remedy that fits what the
        // owner actually typed. This is the catch-all arm for every
        // unrecognised argument, so one unconditional remedy is wrong three
        // different ways, and each arm below exists because of a specific way a
        // reader would be misled:
        //
        // One sentence in three fillings, not three sentences. Every arm is the
        // same imperative -- "pass ... as --port ..." -- and the arms differ
        // only in how much of it we can fill in from what the owner typed.
        //
        // The predicate is **what `service install` will actually accept**, not
        // what the port grammar will parse and not what a `u16` will hold.
        // Those three sets differ, and each gap is a way to hand someone a
        // remedy that fails at the next seam:
        //
        //   * `parse_service_port` parses integer text without imposing a
        //     machine range, so `--port 99999` is accepted here and refused
        //     further in.
        //   * a `u16` holds `0`, which `WindowsServiceAction::arguments`
        //     refuses outright with a message that also blames the journal
        //     path.
        //   * that grammar also strips one leading `+` or `-`, so `+99999` is
        //     number-shaped to an owner *and* to the parser, and only the
        //     accept set separates it from a real port.
        //
        // So: echo the owner's own literal when it is a port this command will
        // take; otherwise, if the argument is number-shaped at all, name the
        // range; otherwise name the flag. `is_port_shaped` deliberately allows
        // the sign the grammar allows, because the arm boundary has to sit on
        // the same axis the grammar does or the gap reopens one character to
        // the left.
        //
        // Echoing the LITERAL, not the parsed value: `007` is the owner's
        // value and `7` is ours. The arm exists to hand back what they typed.
        //
        // Casing is house lowercase: a leading `error:` is sentence prose, not
        // a field label.
        let text =
            solstone_core_system_health::sanitize_os_bytes_for_terminal(value.as_encoded_bytes());
        let literal = value.to_str();
        let accepted_port = literal
            .and_then(|text| text.parse::<u16>().ok())
            .is_some_and(|port| port != 0);
        let remedy = if let (true, Some(port)) = (accepted_port, literal) {
            format!("; pass the port as --port {port}")
        } else if literal.is_some_and(is_port_shaped) {
            "; pass a port between 1 and 65535 as --port PORT".to_owned()
        } else {
            "; pass the port as --port PORT".to_owned()
        };
        Self(format!("error: unexpected argument '{text}'{remedy}"))
    }

    fn unknown_subcommand(value: &OsStr) -> Self {
        // The retained Python owner prints this guidance on two lines. This pure
        // foundation deliberately keeps dynamic failures to one physical line.
        // Both tokens stay lowercase: the command list is part of the sentence,
        // not a title-cased list heading.
        Self(format!(
            "unknown subcommand: {}; available: install, uninstall, start, stop, restart, status, logs",
            solstone_core_system_health::sanitize_os_bytes_for_terminal(value.as_encoded_bytes())
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Render a sanitized service diagnostic as exactly one physical stderr line.
#[doc(hidden)]
#[must_use]
pub fn render_service_diagnostic(diagnostic: &SafeServiceDiagnostic) -> String {
    format!("{}\n", diagnostic.as_str())
}

/// The outcome of parsing the service lifecycle argument grammar.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceParseOutcome {
    Dispatch(ServiceAction),
    Exit {
        code: u8,
        stdout: Option<&'static str>,
        stderr: Option<SafeServiceDiagnostic>,
    },
}

/// Parse the retained `--port` argv grammar without converting argv lossily,
/// defaulting to 5015 when the owner named no port.
#[doc(hidden)]
pub fn parse_service_port_argv(
    args: &[OsString],
) -> Result<solstone_core_operational_logs::ServicePort, SafeServiceDiagnostic> {
    match parse_service_port_argv_opt(args)? {
        Some(port) => Ok(port),
        None => {
            solstone_core_operational_logs::parse_service_port(SERVICE_DEFAULT_PORT).map_err(|_| {
                SafeServiceDiagnostic::invalid_port_value(OsStr::new(SERVICE_DEFAULT_PORT))
            })
        }
    }
}

/// The port a fresh installation takes when nothing else names one.
const SERVICE_DEFAULT_PORT: &str = "5015";

/// Parse `--port` without substituting a default, so a caller can tell
/// "the owner asked for 5015" from "the owner asked for nothing".
#[doc(hidden)]
pub fn parse_service_port_argv_opt(
    args: &[OsString],
) -> Result<Option<solstone_core_operational_logs::ServicePort>, SafeServiceDiagnostic> {
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--port") {
            let Some(value) = args.get(index + 1) else {
                break;
            };
            let Some(value_text) = value.to_str() else {
                return Err(SafeServiceDiagnostic::invalid_port_value(value.as_os_str()));
            };
            return solstone_core_operational_logs::parse_service_port(value_text)
                .map(Some)
                .map_err(|_| SafeServiceDiagnostic::invalid_port_text(value_text));
        }
        if let Some(argument_text) = argument.to_str()
            && let Some(value) = argument_text.strip_prefix("--port=")
        {
            return solstone_core_operational_logs::parse_service_port(value)
                .map(Some)
                .map_err(|_| SafeServiceDiagnostic::invalid_port_text(argument_text));
        }
        index += 1;
    }
    Ok(None)
}

fn parse_service_install_argv(
    args: &[OsString],
) -> Result<
    (
        Option<solstone_core_operational_logs::ServicePort>,
        Option<ServiceInstallationGuardArguments>,
    ),
    SafeServiceDiagnostic,
> {
    let port = parse_service_port_argv_opt(args)?;
    let mut namespace = None;
    let mut id = None;
    let mut generation = None;
    let mut journal_token = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        let Some(name) = argument.to_str() else {
            return Err(SafeServiceDiagnostic::unexpected_install_argument(argument));
        };
        if name == "--port" {
            // Consumed by the port grammar above, value included.
            index += 2;
            continue;
        }
        if name.starts_with("--port=") {
            index += 1;
            continue;
        }
        let field = match name {
            "--installation-namespace" => Some(&mut namespace),
            "--installation-id" => Some(&mut id),
            "--installation-generation" => Some(&mut generation),
            "--installation-journal-token" => Some(&mut journal_token),
            // Everything else used to be skipped in silence, which is how a
            // bare positional port reached the default branch unnoticed.
            _ => return Err(SafeServiceDiagnostic::unexpected_install_argument(argument)),
        };
        let Some(field) = field else {
            index += 1;
            continue;
        };
        let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) else {
            return Err(SafeServiceDiagnostic::incomplete_installation_guard());
        };
        if field.replace(value.to_owned()).is_some() {
            return Err(SafeServiceDiagnostic::incomplete_installation_guard());
        }
        index += 2;
    }
    match (namespace, id, generation, journal_token) {
        (None, None, None, None) => Ok((port, None)),
        (Some(namespace), Some(id), Some(generation), Some(journal_token)) => Ok((
            port,
            Some(ServiceInstallationGuardArguments {
                namespace,
                id,
                generation,
                journal_token,
            }),
        )),
        _ => Err(SafeServiceDiagnostic::incomplete_installation_guard()),
    }
}

/// Parse the complete retained service lifecycle grammar without dispatching it.
#[doc(hidden)]
pub fn parse_service_args(args: &[OsString]) -> ServiceParseOutcome {
    if let [command, rest @ ..] = args
        && command == OsStr::new("logs")
    {
        let parsed = parse_service_logs(rest);
        return ServiceParseOutcome::Dispatch(ServiceAction::Logs {
            follow: parsed.follow,
        });
    }
    if args
        .iter()
        .any(|argument| argument == OsStr::new("--help") || argument == OsStr::new("-h"))
    {
        return ServiceParseOutcome::Exit {
            code: 0,
            stdout: Some(SERVICE_USAGE),
            stderr: None,
        };
    }
    let Some(command) = args.first() else {
        return ServiceParseOutcome::Exit {
            code: 1,
            stdout: Some(SERVICE_USAGE),
            stderr: None,
        };
    };
    let rest = &args[1..];
    if command == OsStr::new("install") {
        return match parse_service_install_argv(rest) {
            Ok((port, installation_guard)) => {
                ServiceParseOutcome::Dispatch(ServiceAction::Install {
                    port,
                    installation_guard,
                })
            }
            Err(stderr) => ServiceParseOutcome::Exit {
                code: 1,
                stdout: None,
                stderr: Some(stderr),
            },
        };
    }
    if command == OsStr::new("up") {
        return ServiceParseOutcome::Dispatch(ServiceAction::Up);
    }
    if command == OsStr::new("restart") {
        return ServiceParseOutcome::Dispatch(ServiceAction::Restart {
            if_installed: rest.iter().any(|argument| argument == "--if-installed"),
        });
    }
    let action = if command == OsStr::new("uninstall") {
        Some(ServiceAction::Uninstall)
    } else if command == OsStr::new("start") {
        Some(ServiceAction::Start)
    } else if command == OsStr::new("stop") {
        Some(ServiceAction::Stop)
    } else if command == OsStr::new("status") {
        Some(ServiceAction::Status)
    } else if command == OsStr::new("down") {
        Some(ServiceAction::Down)
    } else if command == OsStr::new(SERVICE_RESUME_AFTER_UPDATE) && rest.is_empty() {
        Some(ServiceAction::ResumeAfterUpdate)
    } else if command == OsStr::new(SERVICE_BEFORE_UNINSTALL) && rest.is_empty() {
        Some(ServiceAction::BeforeUninstall)
    } else {
        None
    };
    action.map_or_else(
        || ServiceParseOutcome::Exit {
            code: 1,
            stdout: None,
            stderr: Some(SafeServiceDiagnostic::unknown_subcommand(
                command.as_os_str(),
            )),
        },
        ServiceParseOutcome::Dispatch,
    )
}
// SERVICE_ARGS_FOUNDATION_END

enum HealthLogsParse {
    Run(HealthLogsArgs),
    Help(HealthLogsArgs),
}

fn parse_health_logs(args: &[OsString]) -> Result<HealthLogsParse, Box<HealthLogsArgs>> {
    let mut result = HealthLogsArgs {
        count: "5".to_owned(),
        follow: false,
        since: None,
        service: None,
        grep: None,
        verbose: false,
        debug: false,
        value_checks: Vec::new(),
    };
    let mut index = 0;
    let mut saw_unknown = false;
    while index < args.len() {
        let argument = args[index]
            .to_str()
            .ok_or_else(|| Box::new(result.clone()))?;
        if argument == "-h" {
            return Ok(HealthLogsParse::Help(result));
        }
        if argument == "--" {
            return Err(Box::new(result));
        }
        if argument.starts_with("--") {
            match health_logs_long_option(argument).map_err(|_| Box::new(result.clone()))? {
                Some((HealthLogsLongOption::Help, None)) => {
                    return Ok(HealthLogsParse::Help(result));
                }
                Some((HealthLogsLongOption::Verbose, None)) => {
                    result.verbose = true;
                    index += 1;
                    continue;
                }
                Some((HealthLogsLongOption::Debug, None)) => {
                    result.debug = true;
                    index += 1;
                    continue;
                }
                Some((option, attached)) => {
                    let value = attached
                        .map(str::to_owned)
                        .map_or_else(|| detached_health_logs_value(args, index + 1), Ok)
                        .map_err(|_| Box::new(result.clone()))?;
                    match option {
                        HealthLogsLongOption::Since => {
                            result.since = Some(value.clone());
                            result.value_checks.push(HealthLogsValueCheck::Since(value));
                        }
                        HealthLogsLongOption::Service => result.service = Some(value),
                        HealthLogsLongOption::Grep => {
                            result.grep = Some(value.clone());
                            result.value_checks.push(HealthLogsValueCheck::Grep(value));
                        }
                        HealthLogsLongOption::Help
                        | HealthLogsLongOption::Verbose
                        | HealthLogsLongOption::Debug => return Err(Box::new(result)),
                    }
                    index += if attached.is_some() { 1 } else { 2 };
                    continue;
                }
                None => {}
            }
        }
        if argument.starts_with('-') && argument.len() > 2 {
            match parse_health_logs_short_cluster(argument, args, index, &mut result) {
                Err(()) => return Err(Box::new(result)),
                Ok(Some(HealthLogsShortCluster::Help)) => {
                    return Ok(HealthLogsParse::Help(result));
                }
                Ok(Some(HealthLogsShortCluster::Consumed(arguments))) => {
                    index += arguments;
                    continue;
                }
                Ok(None) => {}
            }
        }
        if matches!(argument, "-f") {
            result.follow = true;
            index += 1;
            continue;
        }
        if argument == "-v" {
            result.verbose = true;
            index += 1;
            continue;
        }
        if argument == "-d" {
            result.debug = true;
            index += 1;
            continue;
        }
        if argument == "-c" {
            result.count = detached_health_logs_value(args, index + 1)
                .map_err(|_| Box::new(result.clone()))?;
            result
                .value_checks
                .push(HealthLogsValueCheck::Count(result.count.clone()));
            index += 2;
            continue;
        }
        saw_unknown = true;
        index += 1;
    }
    if saw_unknown {
        Err(Box::new(result))
    } else {
        Ok(HealthLogsParse::Run(result))
    }
}

enum HealthLogsShortCluster {
    Help,
    Consumed(usize),
}

fn parse_health_logs_short_cluster(
    argument: &str,
    args: &[OsString],
    index: usize,
    result: &mut HealthLogsArgs,
) -> Result<Option<HealthLogsShortCluster>, ()> {
    let bytes = argument.as_bytes();
    let mut offset = 1;
    let mut follow = result.follow;
    let mut verbose = result.verbose;
    let mut debug = result.debug;
    while offset < bytes.len() {
        match bytes[offset] {
            b'h' => {
                result.follow = follow;
                result.verbose = verbose;
                result.debug = debug;
                return Ok(Some(HealthLogsShortCluster::Help));
            }
            b'f' => follow = true,
            b'v' => verbose = true,
            b'd' => debug = true,
            b'c' => {
                let attached = &argument[offset + 1..];
                let (value, arguments) = if attached.is_empty() {
                    (detached_health_logs_value(args, index + 1)?, 2)
                } else {
                    (attached.strip_prefix('=').unwrap_or(attached).to_owned(), 1)
                };
                result.count = value.clone();
                result.follow = follow;
                result.verbose = verbose;
                result.debug = debug;
                result.value_checks.push(HealthLogsValueCheck::Count(value));
                return Ok(Some(HealthLogsShortCluster::Consumed(arguments)));
            }
            _ => return Ok(None),
        }
        offset += 1;
    }
    result.follow = follow;
    result.verbose = verbose;
    result.debug = debug;
    Ok(Some(HealthLogsShortCluster::Consumed(1)))
}

#[derive(Clone, Copy)]
enum HealthLogsLongOption {
    Help,
    Since,
    Service,
    Grep,
    Verbose,
    Debug,
}

fn health_logs_long_option(
    argument: &str,
) -> Result<Option<(HealthLogsLongOption, Option<&str>)>, ()> {
    let (option, attached) = argument
        .split_once('=')
        .map_or((argument, None), |(option, value)| (option, Some(value)));
    let options = [
        ("--help", HealthLogsLongOption::Help),
        ("--since", HealthLogsLongOption::Since),
        ("--service", HealthLogsLongOption::Service),
        ("--grep", HealthLogsLongOption::Grep),
        ("--verbose", HealthLogsLongOption::Verbose),
        ("--debug", HealthLogsLongOption::Debug),
    ];
    if let Some((_, exact)) = options.iter().find(|(name, _)| *name == option) {
        return Ok(Some((*exact, attached)));
    }
    let mut matches = options
        .iter()
        .filter(|(name, _)| name.starts_with(option))
        .map(|(_, option)| *option);
    let Some(found) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(());
    }
    Ok(Some((found, attached)))
}

fn detached_health_logs_value(args: &[OsString], index: usize) -> Result<String, ()> {
    let value = args.get(index).and_then(|value| value.to_str()).ok_or(())?;
    if health_logs_detached_value(value) {
        Ok(value.to_owned())
    } else {
        Err(())
    }
}

fn health_logs_detached_value(value: &str) -> bool {
    !value.starts_with('-')
        || value == "-"
        || value.contains(' ')
        || health_logs_negative_number(value)
}

fn health_logs_negative_number(value: &str) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};

    let Some(unsigned) = value.strip_prefix('-') else {
        return false;
    };
    let candidate = unsigned.strip_prefix('.').unwrap_or(unsigned);
    candidate
        .chars()
        .next()
        .is_some_and(|character| get_general_category(character) == GeneralCategory::DecimalNumber)
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
    use super::health_text_fixture::{PortArgv, PortResult, ResultCase, ScalarRecipe};
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn mcp_activity_parses_its_filters_and_passes_target_flags_through() {
        // ⚠ The connection-target flags are read by `parse_mcp_target` from the
        // leftovers, so a stricter unknown-flag guard has to let exactly those
        // three past — rejecting them produced an error that listed `--token`
        // as accepted while refusing it.
        for target in [
            vec!["mcp", "activity", "--token", "probe"],
            vec!["mcp", "activity", "--token=probe"],
        ] {
            let Ok(Command::Mcp(McpCommand::Activity(command))) = evaluate_args(&args(&target))
            else {
                panic!("{target:?} parses as an activity command");
            };
            assert_eq!(
                command.target,
                Some(McpTarget::Token {
                    label: "probe".to_owned()
                })
            );
            assert_eq!(command.limit, DEFAULT_ACTIVITY_LIMIT);
        }

        let Ok(Command::Mcp(McpCommand::Activity(command))) = evaluate_args(&args(&[
            "mcp",
            "activity",
            "--tool",
            "search",
            "--outcome",
            "refused",
            "--day-from",
            "20260901",
            "--day-to=20260915",
            "--limit",
            "5",
            "--json",
        ])) else {
            panic!("the full filter set parses");
        };
        assert_eq!(command.tool.as_deref(), Some("search"));
        assert_eq!(command.outcome.as_deref(), Some("refused"));
        assert_eq!(command.day_from.as_deref(), Some("20260901"));
        assert_eq!(command.day_to.as_deref(), Some("20260915"));
        assert_eq!(command.limit, 5);
        assert!(command.json);
        assert_eq!(command.target, None);
    }

    #[test]
    fn mcp_activity_refuses_a_mistyped_flag_a_bad_day_and_an_out_of_range_limit() {
        for bad in [
            vec!["mcp", "activity", "--tools", "search"],
            vec!["mcp", "activity", "--day-from", "2026-09-01"],
            vec!["mcp", "activity", "--day-to", "202609"],
            vec!["mcp", "activity", "--limit", "0"],
            vec!["mcp", "activity", "--limit", "1001"],
            vec!["mcp", "activity", "--limit", "many"],
        ] {
            assert!(
                matches!(evaluate_args(&args(&bad)), Ok(Command::McpUsage(_))),
                "{bad:?} must be refused rather than silently reinterpreted"
            );
        }
    }

    #[test]
    fn config_help_and_missing_path_usage_name_the_required_path() {
        assert!(CONFIG_HELP.contains("journal config journal PATH"));
        assert!(CONFIG_HELP.contains("journal PATH"));
        assert!(CONFIG_USAGE.contains("journal config journal PATH"));
        assert_eq!(
            evaluate_args(&args(&["config", "journal"])),
            Ok(Command::ConfigUsage)
        );
        assert_eq!(
            evaluate_args(&args(&["config", "journal", "--move"])),
            Ok(Command::ConfigUsage)
        );
    }

    fn scalar_input(recipe: &ScalarRecipe) -> Option<String> {
        match recipe {
            ScalarRecipe::Literal { text } => Some(text.clone()),
            ScalarRecipe::Codepoints { values } => values
                .iter()
                .copied()
                .map(char::from_u32)
                .collect::<Option<String>>(),
            ScalarRecipe::Repeat {
                codepoint,
                count,
                leading,
                separator,
                sign,
                trailing,
            } => {
                let digit = char::from_u32(*codepoint)?;
                let leading = leading
                    .iter()
                    .copied()
                    .map(char::from_u32)
                    .collect::<Option<String>>()?;
                let trailing = trailing
                    .iter()
                    .copied()
                    .map(char::from_u32)
                    .collect::<Option<String>>()?;
                Some(format!(
                    "{leading}{sign}{}{}",
                    std::iter::repeat_n(digit.to_string(), *count as usize)
                        .collect::<Vec<_>>()
                        .join(separator),
                    trailing
                ))
            }
        }
    }

    fn expected_health_count(value: &str) -> solstone_core_operational_logs::ParsedCount {
        match value.parse::<i64>() {
            Ok(value) => solstone_core_operational_logs::ParsedCount::Value(value),
            Err(_) if value.starts_with('-') => {
                solstone_core_operational_logs::ParsedCount::SaturatedNegative
            }
            Err(_) => solstone_core_operational_logs::ParsedCount::SaturatedPositive,
        }
    }

    fn materialize_port_argv(argv: &PortArgv) -> Option<Vec<OsString>> {
        match argv {
            PortArgv::Text { values } => Some(values.iter().map(OsString::from).collect()),
            PortArgv::Codepoints { prefix, values } => {
                let value = values
                    .iter()
                    .copied()
                    .map(char::from_u32)
                    .collect::<Option<String>>()?;
                Some(
                    prefix
                        .iter()
                        .map(OsString::from)
                        .chain(std::iter::once(OsString::from(value)))
                        .collect(),
                )
            }
            PortArgv::Surrogateescape { bytes_hex, prefix } => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;

                    let bytes = (0..bytes_hex.len())
                        .step_by(2)
                        .map(|index| u8::from_str_radix(&bytes_hex[index..index + 2], 16).unwrap())
                        .collect();
                    Some(
                        prefix
                            .iter()
                            .map(OsString::from)
                            .chain(std::iter::once(OsString::from_vec(bytes)))
                            .collect(),
                    )
                }
                #[cfg(not(unix))]
                {
                    let _ = (bytes_hex, prefix);
                    None
                }
            }
        }
    }

    enum PortDiagnosticInput<'a> {
        Separate(&'a OsStr),
        Attached(&'a OsStr),
    }

    fn first_complete_port_argument(args: &[OsString]) -> Option<PortDiagnosticInput<'_>> {
        for (index, argument) in args.iter().enumerate() {
            if argument == "--port" {
                return args
                    .get(index + 1)
                    .map(|value| PortDiagnosticInput::Separate(value.as_os_str()));
            }
            if argument
                .to_str()
                .is_some_and(|value| value.starts_with("--port="))
            {
                return Some(PortDiagnosticInput::Attached(argument.as_os_str()));
            }
        }
        None
    }

    #[test]
    fn canonical_integer_fixture_cases_drive_the_shared_parser_and_health_projection() {
        let fixture = health_text_fixture::health_text_fixture();
        for case in &fixture.scalar_cases {
            let input = scalar_input(&case.recipe);
            match (&case.result, input) {
                (ResultCase::Value { value }, Some(input)) => {
                    let parsed =
                        solstone_core_operational_logs::parse_integer_text(&input).unwrap();
                    assert_eq!(parsed.as_str(), value, "{} canonical", case.id);
                    assert_eq!(
                        solstone_core_operational_logs::parse_health_log_count(&input).unwrap(),
                        expected_health_count(value),
                        "{} health projection",
                        case.id
                    );
                    let argv = vec![OsString::from("--port"), OsString::from(input.as_str())];
                    assert_eq!(
                        parse_service_port_argv(&argv).unwrap().canonical_decimal(),
                        value,
                        "{} service projection",
                        case.id
                    );
                }
                (ResultCase::ValueError, Some(input)) => {
                    assert!(
                        solstone_core_operational_logs::parse_integer_text(&input).is_err(),
                        "{}",
                        case.id
                    );
                    assert!(
                        solstone_core_operational_logs::parse_health_log_count(&input).is_err(),
                        "{} health projection",
                        case.id
                    );
                    let argv = vec![OsString::from("--port"), OsString::from(input.as_str())];
                    assert!(
                        parse_service_port_argv(&argv).is_err(),
                        "{} service projection",
                        case.id
                    );
                }
                (ResultCase::ValueError, None) => assert_eq!(case.id, "lone-surrogate"),
                (ResultCase::Value { .. }, None) => panic!("{} must be materializable", case.id),
            }
        }
        for (scalar, _digit, single, mixed) in &fixture.decimal_cases {
            let scalar = char::from_u32(*scalar).unwrap();
            for (input, expected) in [(scalar.to_string(), single), (format!("1{scalar}2"), mixed)]
            {
                let ResultCase::Value { value } = expected else {
                    panic!("decimal fixture result");
                };
                assert_eq!(
                    solstone_core_operational_logs::parse_integer_text(&input)
                        .unwrap()
                        .as_str(),
                    value
                );
                assert_eq!(
                    solstone_core_operational_logs::parse_health_log_count(&input).unwrap(),
                    expected_health_count(value)
                );
            }
        }
        for (scalar, expected) in &fixture.whitespace_cases {
            let scalar = char::from_u32(*scalar).unwrap();
            let input = format!("{scalar}12{scalar}");
            match expected {
                ResultCase::Value { value } => assert_eq!(
                    solstone_core_operational_logs::parse_integer_text(&input)
                        .unwrap()
                        .as_str(),
                    value
                ),
                ResultCase::ValueError => {
                    assert!(solstone_core_operational_logs::parse_integer_text(&input).is_err())
                }
            }
            if let ResultCase::Value { value } = expected {
                assert_eq!(
                    solstone_core_operational_logs::parse_health_log_count(&input).unwrap(),
                    expected_health_count(value)
                );
            } else {
                assert!(solstone_core_operational_logs::parse_health_log_count(&input).is_err());
            }
        }
    }

    #[test]
    fn canonical_integer_preserves_beyond_i128_fixture_values() {
        let fixture = health_text_fixture::health_text_fixture();
        for case in &fixture.scalar_cases {
            if !matches!(
                case.id.as_str(),
                "beyond-i128-positive" | "beyond-i128-negative"
            ) {
                continue;
            }
            let ResultCase::Value { value } = &case.result else {
                panic!("{case:?}");
            };
            let input = scalar_input(&case.recipe).unwrap();
            assert_eq!(
                solstone_core_operational_logs::parse_integer_text(&input)
                    .unwrap()
                    .as_str(),
                value
            );
        }
    }

    #[test]
    fn health_count_projection_preserves_i64_boundaries() {
        use solstone_core_operational_logs::ParsedCount;

        for (input, expected) in [
            (i64::MAX.to_string(), ParsedCount::Value(i64::MAX)),
            (
                "9223372036854775808".to_owned(),
                ParsedCount::SaturatedPositive,
            ),
            (i64::MIN.to_string(), ParsedCount::Value(i64::MIN)),
            (
                "-9223372036854775809".to_owned(),
                ParsedCount::SaturatedNegative,
            ),
        ] {
            assert_eq!(
                solstone_core_operational_logs::parse_health_log_count(&input),
                Ok(expected)
            );
        }
    }

    #[test]
    fn service_port_fixture_rows_fail_at_first_complete_port() {
        let fixture = health_text_fixture::health_text_fixture();
        assert_eq!(fixture.port_cases.len(), 13);
        let mut non_materializable = 0;
        let mut replayed = 0;
        for case in &fixture.port_cases {
            let Some(argv) = materialize_port_argv(&case.argv) else {
                non_materializable += 1;
                continue;
            };
            replayed += 1;
            match &case.result {
                PortResult::Return { value } => assert_eq!(
                    parse_service_port_argv(&argv).unwrap().canonical_decimal(),
                    value.to_string(),
                    "{}",
                    case.id
                ),
                PortResult::Exit { code, .. } => {
                    let error = parse_service_port_argv(&argv).unwrap_err();
                    assert_eq!(*code, 1, "{}", case.id);
                    assert!(error.as_str().starts_with("error: invalid port '"));
                }
            }
        }
        assert_eq!(non_materializable, 1);
        assert_eq!(replayed, 12);
    }

    #[test]
    fn service_port_errors_are_derived_from_os_sanitization() {
        let fixture = health_text_fixture::health_text_fixture();
        assert_eq!(fixture.port_cases.len(), 13);
        let mut non_materializable = 0;
        let mut replayed = 0;
        for case in &fixture.port_cases {
            let Some(argv) = materialize_port_argv(&case.argv) else {
                non_materializable += 1;
                continue;
            };
            replayed += 1;
            if !matches!(&case.result, PortResult::Exit { .. }) {
                continue;
            }
            let error = parse_service_port_argv(&argv).unwrap_err();
            let selected = first_complete_port_argument(&argv).unwrap();
            let value = match selected {
                PortDiagnosticInput::Separate(value) | PortDiagnosticInput::Attached(value) => {
                    value
                }
            };
            let expected = format!(
                "error: invalid port '{}'\n",
                solstone_core_system_health::sanitize_os_bytes_for_terminal(
                    value.as_encoded_bytes()
                )
            );
            assert_eq!(render_service_diagnostic(&error), expected, "{}", case.id);
        }
        assert_eq!(non_materializable, 1);
        assert_eq!(replayed, 12);
        #[cfg(unix)]
        {
            let argv = vec![
                OsString::from("--port"),
                std::os::unix::ffi::OsStringExt::from_vec(vec![0xff]),
            ];
            assert_eq!(
                render_service_diagnostic(&parse_service_port_argv(&argv).unwrap_err()),
                "error: invalid port '\\xff'\n"
            );
        }
    }

    #[test]
    fn lone_surrogate_fixture_is_non_scalar_provenance_only() {
        assert!(char::from_u32(55296).is_none());
        let fixture = health_text_fixture::health_text_fixture();
        let case = fixture
            .port_cases
            .iter()
            .find(|case| case.id == "lone-surrogate")
            .unwrap();
        assert!(matches!(
            &case.argv,
            PortArgv::Codepoints { values, .. } if values == &[55296]
        ));
        assert!(materialize_port_argv(&case.argv).is_none());
    }

    #[test]
    fn service_args_logs_delegates_to_frozen_service_logs_parser() {
        assert_eq!(
            parse_service_args(&args(&["logs", "ignored", "--follow"])),
            ServiceParseOutcome::Dispatch(ServiceAction::Logs { follow: true })
        );
        assert_eq!(
            parse_service_args(&args(&["logs", "--help"])),
            ServiceParseOutcome::Dispatch(ServiceAction::Logs { follow: false })
        );
    }

    #[test]
    fn service_args_preserves_lifecycle_arm_order_and_streams() {
        assert!(matches!(
            parse_service_args(&args(&["logs", "--help"])),
            ServiceParseOutcome::Dispatch(ServiceAction::Logs { .. })
        ));
        assert_eq!(
            parse_service_args(&args(&["install", "--help"])),
            ServiceParseOutcome::Exit {
                code: 0,
                stdout: Some(SERVICE_USAGE),
                stderr: None,
            }
        );
        assert_eq!(
            parse_service_args(&args(&[])),
            ServiceParseOutcome::Exit {
                code: 1,
                stdout: Some(SERVICE_USAGE),
                stderr: None,
            }
        );
        assert_eq!(
            parse_service_args(&args(&["restart", "ignored", "--if-installed"])),
            ServiceParseOutcome::Dispatch(ServiceAction::Restart { if_installed: true })
        );
        assert_eq!(
            parse_service_args(&args(&["__resume-after-update"])),
            ServiceParseOutcome::Dispatch(ServiceAction::ResumeAfterUpdate)
        );
        assert!(matches!(
            parse_service_args(&args(&["__resume-after-update", "extra"])),
            ServiceParseOutcome::Exit { code: 1, .. }
        ));
        assert_eq!(
            parse_service_args(&args(&["__before-uninstall"])),
            ServiceParseOutcome::Dispatch(ServiceAction::BeforeUninstall)
        );
        assert!(matches!(
            parse_service_args(&args(&["__before-uninstall", "extra"])),
            ServiceParseOutcome::Exit { code: 1, .. }
        ));
        assert!(!SERVICE_USAGE.contains(SERVICE_RESUME_AFTER_UPDATE));
        assert!(!SERVICE_USAGE.contains(SERVICE_BEFORE_UNINSTALL));
        let ServiceParseOutcome::Exit {
            code,
            stdout,
            stderr: Some(stderr),
        } = parse_service_args(&args(&["unknown"]))
        else {
            panic!("unknown outcome");
        };
        assert_eq!(code, 1);
        assert_eq!(stdout, None);
        assert_eq!(
            stderr.as_str(),
            "unknown subcommand: unknown; available: install, uninstall, start, stop, restart, status, logs"
        );
    }

    #[test]
    fn service_args_parses_all_closed_actions() {
        for (argv, expected) in [
            (
                args(&["install", "--port", "7"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Install {
                    port: Some(solstone_core_operational_logs::parse_service_port("7").unwrap()),
                    installation_guard: None,
                }),
            ),
            (
                args(&["uninstall", "ignored"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Uninstall),
            ),
            (
                args(&["start", "ignored"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Start),
            ),
            (
                args(&["stop", "ignored"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Stop),
            ),
            (
                args(&["restart"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Restart {
                    if_installed: false,
                }),
            ),
            (
                args(&["status", "ignored"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Status),
            ),
            (
                args(&["logs"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Logs { follow: false }),
            ),
            (
                args(&["up", "ignored"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Up),
            ),
            (
                args(&["down", "ignored"]),
                ServiceParseOutcome::Dispatch(ServiceAction::Down),
            ),
        ] {
            assert_eq!(parse_service_args(&argv), expected);
        }
    }

    #[test]
    fn service_install_refuses_a_bare_port_instead_of_ignoring_it() {
        // `journal service install 6123` took the port positionally, fell
        // through to the 5015 default, re-registered an established journal
        // there and exited 0. Refusing names the argument and the spelling.
        let ServiceParseOutcome::Exit {
            code: 1,
            stdout: None,
            stderr: Some(stderr),
        } = parse_service_args(&args(&["install", "6123"]))
        else {
            panic!("a bare port must not dispatch");
        };
        assert_eq!(
            stderr.as_str(),
            "error: unexpected argument '6123'; pass the port as --port 6123"
        );
    }

    #[test]
    fn service_install_offers_the_port_remedy_only_when_the_argument_is_a_port() {
        // One imperative in three fillings. Every case here is a value whose
        // remedy would otherwise lead somewhere that fails again:
        //   0      -- a u16, refused by WindowsServiceAction::arguments
        //   99999  -- accepted by the port grammar, refused at registration
        //   +5015  -- the grammar strips the sign, so it IS a real port
        //   +99999 -- the grammar strips the sign and it is still out of range
        //   007    -- a real port; the echo must be the owner's text, not ours
        for (argv, expected) in [
            (
                args(&["install", "--nonsense"]),
                "error: unexpected argument '--nonsense'; pass the port as --port PORT",
            ),
            (
                args(&["install", "99999"]),
                "error: unexpected argument '99999'; pass a port between 1 and 65535 as --port PORT",
            ),
            (
                args(&["install", "65536"]),
                "error: unexpected argument '65536'; pass a port between 1 and 65535 as --port PORT",
            ),
            (
                args(&["install", "0"]),
                "error: unexpected argument '0'; pass a port between 1 and 65535 as --port PORT",
            ),
            (
                args(&["install", "+99999"]),
                "error: unexpected argument '+99999'; pass a port between 1 and 65535 as --port PORT",
            ),
            (
                args(&["install", "-5"]),
                "error: unexpected argument '-5'; pass a port between 1 and 65535 as --port PORT",
            ),
            (
                args(&["install", "5015"]),
                "error: unexpected argument '5015'; pass the port as --port 5015",
            ),
            (
                args(&["install", "65535"]),
                "error: unexpected argument '65535'; pass the port as --port 65535",
            ),
            (
                args(&["install", "007"]),
                "error: unexpected argument '007'; pass the port as --port 007",
            ),
            (
                args(&["install", "+5015"]),
                "error: unexpected argument '+5015'; pass the port as --port +5015",
            ),
            (
                args(&["install", "12x"]),
                "error: unexpected argument '12x'; pass the port as --port PORT",
            ),
        ] {
            let ServiceParseOutcome::Exit {
                code: 1,
                stderr: Some(stderr),
                ..
            } = parse_service_args(&argv)
            else {
                panic!("must not dispatch");
            };
            assert_eq!(stderr.as_str(), expected);
        }
    }

    #[test]
    fn service_install_without_a_port_asks_for_no_particular_port() {
        // The platform then keeps whatever the registration already carries.
        assert_eq!(
            parse_service_args(&args(&["install"])),
            ServiceParseOutcome::Dispatch(ServiceAction::Install {
                port: None,
                installation_guard: None,
            })
        );
        assert_eq!(
            parse_service_args(&args(&["install", "--port=6123"])),
            ServiceParseOutcome::Dispatch(ServiceAction::Install {
                port: Some(solstone_core_operational_logs::parse_service_port("6123").unwrap()),
                installation_guard: None,
            })
        );
    }

    #[test]
    fn service_install_refuses_an_unknown_flag_rather_than_skipping_it() {
        for argv in [
            args(&["install", "--nonsense"]),
            args(&["install", "--port", "6123", "--nonsense", "x"]),
        ] {
            let ServiceParseOutcome::Exit {
                code: 1,
                stderr: Some(stderr),
                ..
            } = parse_service_args(&argv)
            else {
                panic!("an unknown flag must not dispatch");
            };
            assert_eq!(
                stderr.as_str(),
                "error: unexpected argument '--nonsense'; pass the port as --port PORT",
                "an unknown flag must not be handed a port remedy naming itself"
            );
        }
    }

    #[test]
    fn service_install_identity_arguments_are_all_or_nothing() {
        let all = args(&[
            "install",
            "--installation-namespace",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "--installation-id",
            "0123456789abcdef0123456789abcdef",
            "--installation-generation",
            "1",
            "--installation-journal-token",
            "2f6a6f75726e616c",
        ]);
        assert!(matches!(
            parse_service_args(&all),
            ServiceParseOutcome::Dispatch(ServiceAction::Install {
                installation_guard: Some(_),
                ..
            })
        ));
        let partial = args(&[
            "install",
            "--installation-id",
            "0123456789abcdef0123456789abcdef",
        ]);
        let ServiceParseOutcome::Exit {
            code: 1,
            stderr: Some(stderr),
            ..
        } = parse_service_args(&partial)
        else {
            panic!("partial guard should fail");
        };
        assert_eq!(
            stderr.as_str(),
            "Error: installation identity arguments must be supplied together"
        );
    }

    #[cfg(unix)]
    #[test]
    fn service_port_invalid_os_bytes_are_reversible() {
        use std::os::unix::ffi::OsStringExt;

        let argv = vec![OsString::from("--port"), OsString::from_vec(vec![0xff])];
        assert_eq!(
            render_service_diagnostic(&parse_service_port_argv(&argv).unwrap_err()),
            "error: invalid port '\\xff'\n"
        );
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
    fn parses_required_path_only_navigate_arguments() {
        for (values, expected_path) in [
            (&["navigate", "/home"][..], "/home"),
            (&["navigate", "--", "/home"][..], "/home"),
            (&["navigate", "--", "-weird"][..], "-weird"),
            (&["navigate", "--", "-fwork"][..], "-fwork"),
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Navigate {
                    path: expected_path.to_owned(),
                }),
                "{values:?}"
            );
        }
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
            &["navigate"][..],
            &["navigate", "--"][..],
            &["navigate", "--nonsense"][..],
            &["navigate", "-x"][..],
            &["navigate", "-weird"][..],
            &["navigate", "/a", "/b"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::NavigateUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn navigate_facet_options_are_rejected_with_workspace_local_guidance() {
        for (values, option) in [
            (&["navigate", "--facet", "work"][..], "--facet"),
            (&["navigate", "/app/work", "--facet=work"][..], "--facet"),
            (&["navigate", "-f", "work"][..], "-f"),
            (&["navigate", "/app/work", "-fwork"][..], "-f"),
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::NavigateFacetRetired(option)),
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
            &["convey", "--journal", "/tmp/journal", "--port", "--journal"][..],
        ] {
            assert!(
                matches!(evaluate_args(&args(values)), Ok(Command::ConveyUsage(_))),
                "{values:?}"
            );
        }
        assert_eq!(
            evaluate_args(&args(&["convey", "--port", "5015", "--port", "5016"])),
            Ok(Command::Convey(ConveyOptions {
                port: 5016,
                journal_override: None,
            }))
        );
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
    fn rejects_retired_mlx_install_verbs() {
        for values in [
            &["local", "install", "fingerprint", "mlx"][..],
            &["local", "install", "inspect", "mlx"][..],
            &["local", "install", "run", "mlx"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError));
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
        let owner = |tail: &[&str]| {
            let mut values = vec![
                OsString::from(JOURNAL_BRAIN_OWNER_SENTINEL),
                OsString::from("brain"),
            ];
            values.extend(tail.iter().map(OsString::from));
            evaluate_args(&values)
        };
        assert_eq!(
            owner(&["status", "--json"]),
            Ok(Command::JournalBrainOwner(
                JournalBrainOwnerCommand::Status { json: true }
            ))
        );
        assert_eq!(
            owner(&[
                "-v",
                "refresh",
                "--json",
                "--expected-fingerprint",
                hash,
                "--expected-fingerprint",
                "not-a-sha",
                "--expected-active-fingerprint",
                "--expect-active-fingerprint-absent"
            ]),
            Ok(Command::JournalBrainOwner(
                JournalBrainOwnerCommand::Refresh(JournalBrainRefreshOptions {
                    json: true,
                    expected_fingerprint: Some("not-a-sha".to_owned()),
                    expected_active_fingerprint: true,
                    expect_active_fingerprint_absent: true
                })
            ))
        );
        assert_eq!(
            owner(&[
                "renew-prerequisites",
                "--json",
                "--expected-fingerprint",
                hash
            ]),
            Ok(Command::JournalBrainOwner(
                JournalBrainOwnerCommand::RenewPrerequisites {
                    json: true,
                    expected_fingerprint: Some(hash.to_owned())
                }
            ))
        );
    }

    #[test]
    fn rejects_invalid_brain_args() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(evaluate_args(&args(&["brain", "status"])), Err(UsageError));
        let owner_internal = vec![
            OsString::from(JOURNAL_BRAIN_OWNER_SENTINEL),
            OsString::from("brain"),
            OsString::from("inspect"),
        ];
        assert_eq!(
            evaluate_args(&owner_internal),
            Ok(Command::JournalBrainOwner(JournalBrainOwnerCommand::Usage))
        );
        use std::os::unix::ffi::OsStringExt;
        let mut owner_non_utf8_name = vec![
            OsString::from(JOURNAL_BRAIN_OWNER_SENTINEL),
            OsString::from("brain"),
            OsString::from("status"),
        ];
        owner_non_utf8_name.push(OsString::from_vec(vec![b'-', b'-', 0xff]));
        assert_eq!(
            evaluate_args(&owner_non_utf8_name),
            Ok(Command::JournalBrainOwner(JournalBrainOwnerCommand::Usage))
        );
        let owner_non_utf8_value = vec![
            OsString::from(JOURNAL_BRAIN_OWNER_SENTINEL),
            OsString::from("brain"),
            OsString::from("refresh"),
            OsString::from("--expected-fingerprint"),
            OsString::from_vec(vec![0xff]),
        ];
        assert!(matches!(
            evaluate_args(&owner_non_utf8_value),
            Ok(Command::JournalBrainOwner(
                JournalBrainOwnerCommand::Refresh(JournalBrainRefreshOptions {
                    expected_fingerprint: Some(_),
                    ..
                })
            ))
        ));
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
            assert!(
                matches!(evaluate_args(&args(values)), Ok(Command::SplUsage(_))),
                "{values:?}"
            );
        }
    }

    #[test]
    fn rejects_spl_service_extra_args() {
        for values in [
            &["spl", "service", "extra"][..],
            &["spl", "service", "service"][..],
        ] {
            assert!(
                matches!(evaluate_args(&args(values)), Ok(Command::SplUsage(_))),
                "{values:?}"
            );
        }
    }

    #[test]
    fn rejects_incomplete_unknown_and_extra_spl_args() {
        for values in [&["spl"][..], &["spl", "unknown"][..]] {
            assert!(
                matches!(evaluate_args(&args(values)), Ok(Command::SplUsage(_))),
                "{values:?}"
            );
        }
    }

    #[test]
    fn spl_help_flag_is_help_not_an_invalid_choice() {
        for values in [
            &["spl", "--help"][..],
            &["spl", "-h"][..],
            &["spl", "service", "--help"][..],
            &["spl", "service", "-h"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::SplHelp),
                "{values:?}"
            );
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
            "doctor",
            "journal-path",
            "indexer",
            "journal-config",
            "speaker-transcript-write",
            "speaker-resolve",
            "local",
            "generate",
            "cogitate",
            "brain",
            "body",
            "grab",
            "spl",
            "supervisor",
            "navigate",
            "identity",
            "settings",
            "contract",
            "facet-candidates",
            "install-models",
            "install-provider",
            "thinking",
            "streams",
            "importer",
            "segment",
            "backup",
            "journal-stats",
            "reprocess",
            "backfill-processing-records",
        ] {
            assert!(
                USAGE.contains(&format!("solstone-core {command}")),
                "USAGE does not list `{command}`"
            );
        }
        assert!(USAGE.contains("journal convey"));
        assert!(USAGE.contains("journal schedule"));
        assert!(USAGE.starts_with("Usage:\n"));
    }

    #[test]
    fn parses_contract_leaves_before_execution() {
        assert_eq!(
            evaluate_args(&args(&["contract", "build", "--check", "--root=/tmp/root"])),
            Ok(Command::Contract(ContractCommand::Build {
                check: true,
                root: Some(PathBuf::from("/tmp/root")),
            }))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "contract",
                "check",
                "--journal",
                "/one",
                "--journal=/two",
                "--root",
                "/root",
            ])),
            Ok(Command::Contract(ContractCommand::Check {
                journals: vec![PathBuf::from("/one"), PathBuf::from("/two")],
                root: Some(PathBuf::from("/root")),
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["contract", "--nonsense"])),
            Ok(Command::ContractUsage)
        );
        assert_eq!(
            evaluate_args(&args(&["contract", "build", "--nonsense"])),
            Ok(Command::ContractBuildUsage)
        );
        assert_eq!(
            evaluate_args(&args(&["contract", "check", "--nonsense"])),
            Ok(Command::ContractCheckUsage)
        );
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
                required_only: false,
                variant: InstallModelsVariant::Auto,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["install-models", "--check", "--variant=cpu"])),
            Ok(Command::InstallModels(InstallModelsOptions {
                check: true,
                force: false,
                required_only: false,
                variant: InstallModelsVariant::Cpu,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["install-models", "--force", "--variant", "coreml"])),
            Ok(Command::InstallModels(InstallModelsOptions {
                check: false,
                force: true,
                required_only: false,
                variant: InstallModelsVariant::Coreml,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["install-models", "--required-only"])),
            Ok(Command::InstallModels(InstallModelsOptions {
                check: false,
                force: false,
                required_only: true,
                variant: InstallModelsVariant::Auto,
            }))
        );
        for values in [
            &["install-models", "--check", "--force"][..],
            &["install-models", "--variant"][..],
            &["install-models", "--variant", "bad"][..],
            &["install-models", "--variant", "cpu", "--variant", "cuda"][..],
            &["install-models", "--required-only", "--required-only"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::InstallModelsUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn check_rejects_unknown_and_help_flags_with_verb_results() {
        assert_eq!(
            evaluate_args(&args(&["check", "--nonsense"])),
            Ok(Command::CheckUsage)
        );
        for values in [
            &["check", "--help"][..],
            &["check", "-h"][..],
            &["check", "--json", "--help"][..],
            &["check", "--nonsense", "--help"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::CheckHelp),
                "{values:?}"
            );
        }
    }

    #[test]
    fn doctor_rejects_unknown_and_help_flags_with_verb_results() {
        assert!(matches!(
            evaluate_args(&args(&["doctor", "--nonsense"])),
            Ok(Command::DoctorUsage(_))
        ));
        for values in [
            &["doctor", "--help"][..],
            &["doctor", "-h"][..],
            &["doctor", "--json", "--help"][..],
            &["doctor", "--nonsense", "--help"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::DoctorHelp),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parses_install_provider_name_without_validating_it() {
        for name in ["parakeet", "local", "bogus"] {
            assert_eq!(
                evaluate_args(&args(&["install-provider", name])),
                Ok(Command::InstallProvider(InstallProviderOptions {
                    name: name.to_owned(),
                })),
                "{name}"
            );
        }
        for values in [
            &["install-provider"][..],
            &["install-provider", "parakeet", "extra"][..],
            &["install-provider", "--wat"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::InstallProviderUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn install_provider_rejects_unknown_and_serves_help_with_verb_results() {
        for values in [
            &["install-provider", "--help"][..],
            &["install-provider", "-h"][..],
            &["install-provider", "bogus", "--help"][..],
            &["install-provider", "--wat", "--help"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::InstallProviderHelp),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parses_thinking_group_help_and_usage() {
        for values in [&["thinking", "--help"][..], &["thinking", "-h"][..]] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::ThinkingHelp),
                "{values:?}"
            );
        }
        for values in [&["thinking"][..], &["thinking", "bogus"][..]] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::ThinkingUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parses_thinking_set_lane_help_at_the_leaf() {
        for values in [
            &["thinking", "set-lane", "--help"][..],
            &["thinking", "set-lane", "local", "--help"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Thinking(ThinkingCommand::SetLaneHelp)),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parses_thinking_set_lane_usage_for_malformed_forms() {
        for values in [
            &["thinking", "set-lane"][..],
            &["thinking", "set-lane", "--journal", "/x"][..],
            &["thinking", "set-lane", "byo", "--provider"][..],
            &[
                "thinking",
                "set-lane",
                "byo",
                "--provider",
                "openai",
                "--provider",
                "google",
            ][..],
            &["thinking", "set-lane", "byo", "--wat"][..],
            &["thinking", "set-lane", "byo", "extra"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Thinking(ThinkingCommand::SetLaneUsage)),
                "{values:?}"
            );
        }
    }

    #[test]
    fn parses_thinking_set_lane_without_validating_the_lane() {
        assert_eq!(
            evaluate_args(&args(&["thinking", "set-lane", "nope"])),
            Ok(Command::Thinking(ThinkingCommand::SetLane(
                ThinkingSetLaneOptions {
                    lane: "nope".to_owned(),
                    provider: None,
                    model: None,
                    journal_override: None,
                }
            )))
        );
    }

    #[test]
    fn parses_thinking_set_lane_provider_model_and_journal() {
        assert_eq!(
            evaluate_args(&args(&[
                "thinking",
                "set-lane",
                "byo",
                "--provider",
                "openai",
                "--model",
                "gpt-5",
                "--journal",
                "/j",
            ])),
            Ok(Command::Thinking(ThinkingCommand::SetLane(
                ThinkingSetLaneOptions {
                    lane: "byo".to_owned(),
                    provider: Some("openai".to_owned()),
                    model: Some("gpt-5".to_owned()),
                    journal_override: Some("/j".into()),
                }
            )))
        );
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
                no_schedule: false,
                no_convey: false,
                no_cortex: false,
                no_spl: false,
                direct_port: None,
                hosted_parent: false,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["supervisor", "--journal"])),
            Ok(Command::SupervisorUsage)
        );
        assert_eq!(
            evaluate_args(&args(&["supervisor", "--wat"])),
            Ok(Command::SupervisorUsage)
        );
        assert_eq!(
            evaluate_args(&args(&["start", "--journal", "/tmp/journal"])),
            Ok(Command::Supervisor(SupervisorOptions {
                port: 0,
                journal_override: Some(OsString::from("/tmp/journal")),
                no_daily: false,
                no_schedule: false,
                no_convey: false,
                no_cortex: false,
                no_spl: false,
                direct_port: None,
                hosted_parent: false,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["start", "--wat"])),
            Ok(Command::StartUsage)
        );
        assert_eq!(
            evaluate_args(&args(&["start", "--help"])),
            Ok(Command::StartHelp)
        );
        assert_eq!(
            evaluate_args(&args(&["up", "--help"])),
            Ok(Command::Service(ServiceParseOutcome::Exit {
                code: 0,
                stdout: Some(UP_HELP),
                stderr: None,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["down", "-h"])),
            Ok(Command::Service(ServiceParseOutcome::Exit {
                code: 0,
                stdout: Some(DOWN_HELP),
                stderr: None,
            }))
        );
    }

    #[test]
    fn parses_bare_health_flags_and_health_logs_grammar() {
        assert_eq!(
            evaluate_args(&args(&["health", "-v", "--debug"])),
            Ok(Command::Health {
                verbose: true,
                debug: true,
            })
        );
        for values in [&["health", "--wat"][..], &["health", "-v", "--verbose"][..]] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::HealthUsage),
                "{values:?}"
            );
        }
        assert_eq!(
            evaluate_args(&args(&["health", "--help"])),
            Ok(Command::HealthHelp)
        );
        assert_eq!(
            evaluate_args(&args(&["health", "logs"])),
            Ok(Command::HealthLogs(HealthLogsArgs {
                count: "5".into(),
                follow: false,
                since: None,
                service: None,
                grep: None,
                verbose: false,
                debug: false,
                value_checks: Vec::new(),
            }))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "health",
                "logs",
                "-c",
                "10",
                "-f",
                "--since=30m",
                "--service",
                "observer",
                "--grep=error",
                "-v",
                "-d"
            ])),
            Ok(Command::HealthLogs(HealthLogsArgs {
                count: "10".into(),
                follow: true,
                since: Some("30m".into()),
                service: Some("observer".into()),
                grep: Some("error".into()),
                verbose: true,
                debug: true,
                value_checks: vec![
                    HealthLogsValueCheck::Count("10".into()),
                    HealthLogsValueCheck::Since("30m".into()),
                    HealthLogsValueCheck::Grep("error".into()),
                ],
            }))
        );
        for values in [
            ["health", "logs", "-c5"].as_slice(),
            ["health", "logs", "-c=5"].as_slice(),
        ] {
            assert!(
                matches!(evaluate_args(&args(values)), Ok(Command::HealthLogs(HealthLogsArgs { count, .. })) if count == "5")
            );
        }
        for values in [
            ["health", "logs", "--nonsense"].as_slice(),
            ["health", "logs", "--"].as_slice(),
            ["health", "logs", "--service"].as_slice(),
        ] {
            assert!(matches!(
                evaluate_args(&args(values)),
                Ok(Command::HealthLogsUsage(_))
            ));
        }
        for values in [
            ["health", "logs", "--help"].as_slice(),
            ["health", "logs", "-h"].as_slice(),
        ] {
            assert!(matches!(
                evaluate_args(&args(values)),
                Ok(Command::HealthLogsHelp(_))
            ));
        }
        assert!(
            matches!(evaluate_args(&args(&["health", "logs", "-c", "5", "-c", "10"])), Ok(Command::HealthLogs(HealthLogsArgs { count, .. })) if count == "10")
        );
        assert!(
            matches!(evaluate_args(&args(&["health", "logs", "--service=first", "--service", "last", "-f", "-f", "-v", "-v", "-d", "-d"])), Ok(Command::HealthLogs(HealthLogsArgs { service: Some(service), follow: true, verbose: true, debug: true, .. })) if service == "last")
        );
    }

    #[test]
    fn parses_top_flags_and_uses_command_specific_usage() {
        assert_eq!(
            evaluate_args(&args(&["top", "-v", "--debug"])),
            Ok(Command::Top {
                verbose: true,
                debug: true,
            })
        );
        assert_eq!(
            evaluate_args(&args(&["top", "--bogus"])),
            Ok(Command::TopUsage)
        );
        assert_eq!(
            evaluate_args(&args(&["top", "--help"])),
            Ok(Command::TopHelp)
        );
    }

    #[test]
    fn health_logs_preserves_value_order_and_argparse_help_precedence() {
        let command = evaluate_args(&args(&[
            "health", "logs", "-c", "bad", "-c", "5", "--since", "bad", "--grep", "(", "--help",
        ]))
        .unwrap();
        let Command::HealthLogsHelp(parsed) = command else {
            panic!("expected ordered health logs help carrier");
        };
        assert_eq!(
            parsed.value_checks,
            [
                HealthLogsValueCheck::Count("bad".into()),
                HealthLogsValueCheck::Count("5".into()),
                HealthLogsValueCheck::Since("bad".into()),
                HealthLogsValueCheck::Grep("(".into()),
            ]
        );

        let Command::HealthLogsHelp(parsed) =
            evaluate_args(&args(&["health", "logs", "--help", "-c", "bad"])).unwrap()
        else {
            panic!("help must stop parsing");
        };
        assert!(parsed.value_checks.is_empty());
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--bogus", "--help"])),
            Ok(Command::HealthLogsHelp(_))
        ));
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--bogus"])),
            Ok(Command::HealthLogsUsage(_))
        ));

        for values in [
            ["health", "logs", "-c", "bad", "--bogus"].as_slice(),
            ["health", "logs", "--bogus", "-c", "bad"].as_slice(),
            ["health", "logs", "-c", "bad", "--service"].as_slice(),
            ["health", "logs", "-c", "bad", "--s", "x"].as_slice(),
        ] {
            assert!(
                matches!(
                    evaluate_args(&args(values)),
                    Ok(Command::HealthLogsUsage(HealthLogsArgs { value_checks, .. }))
                        if value_checks == [HealthLogsValueCheck::Count("bad".into())]
                ),
                "{values:?}"
            );
        }
        assert!(matches!(
            evaluate_args(&args(&[
                "health", "logs", "--service", "-f", "-c", "bad"
            ])),
            Ok(Command::HealthLogsUsage(HealthLogsArgs { value_checks, .. }))
                if value_checks.is_empty()
        ));
    }

    #[test]
    fn health_logs_detached_values_match_argparse_optional_classification() {
        for value in [
            "plain", "-", "-1", "-.5", "-١", "- name", "-1e2", "-1.", "-0x1", "-.5x", "-.١x",
        ] {
            assert!(
                matches!(
                    evaluate_args(&args(&["health", "logs", "--service", value])),
                    Ok(Command::HealthLogs(HealthLogsArgs { service: Some(actual), .. })) if actual == value
                ),
                "{value:?}"
            );
        }
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--service=-f"])),
            Ok(Command::HealthLogs(HealthLogsArgs { service: Some(value), .. })) if value == "-f"
        ));
        for value in ["-f", "--bogus", "-name", "-.x", "--", "--help"] {
            assert!(
                matches!(
                    evaluate_args(&args(&["health", "logs", "--service", value])),
                    Ok(Command::HealthLogsUsage(_))
                ),
                "{value:?}"
            );
        }
    }

    #[test]
    fn health_logs_end_of_options_cannot_activate_later_help() {
        for values in [
            ["health", "logs", "--"].as_slice(),
            ["health", "logs", "--", "--help"].as_slice(),
            ["health", "logs", "--service", "--"].as_slice(),
        ] {
            assert!(
                matches!(
                    evaluate_args(&args(values)),
                    Ok(Command::HealthLogsUsage(_))
                ),
                "{values:?}"
            );
        }
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--help", "--"])),
            Ok(Command::HealthLogsHelp(_))
        ));
    }

    #[test]
    fn health_logs_long_options_use_argparse_unique_prefixes() {
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--serv=x"])),
            Ok(Command::HealthLogs(HealthLogsArgs { service: Some(value), .. })) if value == "x"
        ));
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--gre=x"])),
            Ok(Command::HealthLogs(HealthLogsArgs { grep: Some(value), .. })) if value == "x"
        ));
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--ver", "--deb"])),
            Ok(Command::HealthLogs(HealthLogsArgs {
                verbose: true,
                debug: true,
                ..
            }))
        ));
        for values in [
            ["health", "logs", "--s", "x", "--help"].as_slice(),
            ["health", "logs", "--s=x", "--help"].as_slice(),
            ["health", "logs", "--ver=x"].as_slice(),
        ] {
            assert!(
                matches!(
                    evaluate_args(&args(values)),
                    Ok(Command::HealthLogsUsage(_))
                ),
                "{values:?}"
            );
        }
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--help", "--s", "x"])),
            Ok(Command::HealthLogsHelp(_))
        ));
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "--fo", "--help"])),
            Ok(Command::HealthLogsHelp(_))
        ));
    }

    #[test]
    fn health_logs_short_clusters_match_argparse_consumption() {
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "-vf", "-df"])),
            Ok(Command::HealthLogs(HealthLogsArgs {
                follow: true,
                verbose: true,
                debug: true,
                ..
            }))
        ));
        for value in ["-fc5", "-vc5"] {
            assert!(matches!(
                evaluate_args(&args(&["health", "logs", value])),
                Ok(Command::HealthLogs(HealthLogsArgs { count, .. })) if count == "5"
            ));
        }
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "-vh"])),
            Ok(Command::HealthLogsHelp(HealthLogsArgs {
                verbose: true,
                ..
            }))
        ));
        assert!(matches!(
            evaluate_args(&args(&["health", "logs", "-x", "--help"])),
            Ok(Command::HealthLogsHelp(_))
        ));
        for values in [
            ["health", "logs", "-vx"].as_slice(),
            ["health", "logs", "-cv"].as_slice(),
        ] {
            let command = evaluate_args(&args(values)).unwrap();
            if values[2] == "-vx" {
                assert!(matches!(command, Command::HealthLogsUsage(_)));
            } else {
                assert!(matches!(
                    command,
                    Command::HealthLogs(HealthLogsArgs {
                        value_checks,
                        ..
                    }) if value_checks == [HealthLogsValueCheck::Count("v".into())]
                ));
            }
        }
        let Command::HealthLogsHelp(parsed) =
            evaluate_args(&args(&["health", "logs", "-vx", "--help"])).unwrap()
        else {
            panic!("unknown short cluster must defer to later help");
        };
        assert!(!parsed.verbose, "unknown cluster must be transactional");
        assert!(matches!(
            evaluate_args(&args(&[
                "health", "logs", "-vx", "-c", "bad", "--help"
            ])),
            Ok(Command::HealthLogsHelp(HealthLogsArgs { value_checks, .. }))
                if value_checks == [HealthLogsValueCheck::Count("bad".into())]
        ));
    }

    #[cfg(unix)]
    #[test]
    fn health_logs_non_utf8_arguments_are_usage_carriers() {
        use std::os::unix::ffi::OsStringExt;

        assert!(matches!(
            evaluate_args(&[
                OsString::from("health"),
                OsString::from("logs"),
                OsString::from_vec(vec![0xff]),
            ]),
            Ok(Command::HealthLogsUsage(_))
        ));
    }

    #[test]
    fn service_logs_selects_follow_anywhere_and_ignores_other_trailing_tokens() {
        assert_eq!(
            evaluate_args(&args(&["service", "logs"])),
            Ok(Command::Service(ServiceParseOutcome::Dispatch(
                ServiceAction::Logs { follow: false }
            )))
        );
        for values in [
            &["service", "logs", "--help"][..],
            &["service", "logs", "ignored", "-x"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Service(ServiceParseOutcome::Dispatch(
                    ServiceAction::Logs { follow: false }
                ))),
                "{values:?}"
            );
        }
        for values in [
            &["service", "logs", "-f"][..],
            &["service", "logs", "ignored", "--follow", "--help"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::Service(ServiceParseOutcome::Dispatch(
                    ServiceAction::Logs { follow: true }
                ))),
                "{values:?}"
            );
        }
        assert!(matches!(
            evaluate_args(&args(&["service"])),
            Ok(Command::Service(ServiceParseOutcome::Exit {
                code: 1,
                stdout: Some(SERVICE_USAGE),
                stderr: None,
            }))
        ));
        assert!(matches!(
            evaluate_args(&args(&["service", "--help"])),
            Ok(Command::Service(ServiceParseOutcome::Exit {
                code: 0,
                stdout: Some(SERVICE_USAGE),
                stderr: None,
            }))
        ));
        assert_eq!(
            evaluate_args(&args(&["service", "status"])),
            Ok(Command::Service(ServiceParseOutcome::Dispatch(
                ServiceAction::Status
            )))
        );
    }

    #[cfg(unix)]
    #[test]
    fn service_logs_keeps_ignored_non_utf8_opaque_but_rejects_route_bytes() {
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            evaluate_args(&[
                OsString::from("service"),
                OsString::from("logs"),
                OsString::from_vec(vec![0xff]),
            ]),
            Ok(Command::Service(ServiceParseOutcome::Dispatch(
                ServiceAction::Logs { follow: false }
            )))
        );
        assert!(evaluate_args(&[OsString::from_vec(vec![0xff]), OsString::from("logs")]).is_err());
        assert!(matches!(
            evaluate_args(&[OsString::from("service"), OsString::from_vec(vec![0xff])]),
            Ok(Command::Service(ServiceParseOutcome::Exit {
                code: 1,
                stdout: None,
                stderr: Some(_),
            }))
        ));
    }

    #[test]
    fn parses_supervisor_stack_options_in_any_order() {
        assert_eq!(
            evaluate_args(&args(&[
                "supervisor",
                "5015",
                "--no-spl",
                "--journal",
                "/tmp/journal",
                "--no-convey",
                "--no-cortex",
                "--no-daily",
                "--no-schedule",
            ])),
            Ok(Command::Supervisor(SupervisorOptions {
                port: 5015,
                journal_override: Some(OsString::from("/tmp/journal")),
                no_daily: true,
                no_schedule: true,
                no_convey: true,
                no_cortex: true,
                no_spl: true,
                direct_port: None,
                hosted_parent: false,
            }))
        );
    }

    #[test]
    fn parses_supervisor_direct_port() {
        assert_eq!(
            evaluate_args(&args(&["supervisor"])),
            Ok(Command::Supervisor(SupervisorOptions {
                port: 0,
                journal_override: None,
                no_daily: false,
                no_schedule: false,
                no_convey: false,
                no_cortex: false,
                no_spl: false,
                direct_port: None,
                hosted_parent: false,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["supervisor", "--direct-port", "9000"])),
            Ok(Command::Supervisor(SupervisorOptions {
                port: 0,
                journal_override: None,
                no_daily: false,
                no_schedule: false,
                no_convey: false,
                no_cortex: false,
                no_spl: false,
                direct_port: Some(9000),
                hosted_parent: false,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["supervisor", "--direct-port=9000"])),
            Ok(Command::Supervisor(SupervisorOptions {
                port: 0,
                journal_override: None,
                no_daily: false,
                no_schedule: false,
                no_convey: false,
                no_cortex: false,
                no_spl: false,
                direct_port: Some(9000),
                hosted_parent: false,
            }))
        );
        for (values, needle) in [
            (
                &["supervisor", "--direct-port"][..],
                "expected one argument",
            ),
            (
                &["supervisor", "--direct-port", "not-a-port"][..],
                "invalid int value",
            ),
            (&["supervisor", "--direct-port", "0"][..], "must be between"),
            (
                &["supervisor", "--direct-port", "65536"][..],
                "must be between",
            ),
            (
                &[
                    "supervisor",
                    "--direct-port",
                    "9000",
                    "--direct-port",
                    "9001",
                ][..],
                "cannot be repeated",
            ),
        ] {
            match evaluate_args(&args(values)) {
                Ok(Command::SupervisorInvalid(error)) => {
                    assert!(error.0.contains("--direct-port"), "{values:?}: {}", error.0);
                    assert!(error.0.contains(needle), "{values:?}: {}", error.0);
                }
                other => panic!("{values:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn start_hosted_parent_preserves_every_other_supervisor_option() {
        let no_flags = [
            (1, "--no-daily"),
            (2, "--no-schedule"),
            (4, "--no-convey"),
            (8, "--no-cortex"),
            (16, "--no-spl"),
        ];

        for no_flag_mask in 0..32 {
            for journal_override in [false, true] {
                for direct_port in [false, true] {
                    for convey_port in [None, Some("5015")] {
                        let mut plain = vec![OsString::from("start")];
                        for (mask, flag) in no_flags {
                            if no_flag_mask & mask != 0 {
                                plain.push(OsString::from(flag));
                            }
                        }
                        if journal_override {
                            plain.extend([
                                OsString::from("--journal"),
                                OsString::from("/tmp/journal"),
                            ]);
                        }
                        if direct_port {
                            plain.extend([OsString::from("--direct-port"), OsString::from("9000")]);
                        }
                        if let Some(port) = convey_port {
                            plain.push(OsString::from(port));
                        }

                        let plain_options = match evaluate_args(&plain) {
                            Ok(Command::Supervisor(options)) => options,
                            other => panic!("{plain:?}: {other:?}"),
                        };
                        let mut hosted = plain.clone();
                        hosted.insert(1, OsString::from("--hosted-parent"));
                        let hosted_options = match evaluate_args(&hosted) {
                            Ok(Command::Supervisor(options)) => options,
                            other => panic!("{hosted:?}: {other:?}"),
                        };

                        assert!(!plain_options.hosted_parent, "{plain:?}");
                        assert_eq!(
                            hosted_options,
                            SupervisorOptions {
                                hosted_parent: true,
                                ..plain_options
                            },
                            "{plain:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn start_hosted_parent_accepts_a_following_convey_port() {
        match evaluate_args(&args(&["start", "--hosted-parent", "5015"])) {
            Ok(Command::Supervisor(options)) => {
                assert_eq!(options.port, 5015);
                assert!(options.hosted_parent);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hosted_parent_is_start_only_and_app_supervised_remains_unknown() {
        for values in [
            &["supervisor", "--hosted-parent"][..],
            &["supervisor", "--hosted-parent", "5015"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::SupervisorUsage),
                "{values:?}"
            );
        }
        for values in [
            &["start", "--hosted-parent", "--hosted-parent"][..],
            &["start", "--app-supervised", "5015"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::StartUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn hosted_parent_is_documented_only_for_start() {
        for text in [START_USAGE, START_HELP] {
            assert!(text.contains("--hosted-parent"));
        }
        for text in [SUPERVISOR_USAGE, SUPERVISOR_HELP] {
            assert!(!text.contains("--hosted-parent"));
        }
    }

    #[test]
    fn rejects_invalid_supervisor_stack_options() {
        for values in [
            &["supervisor", "--no-convey", "--no-convey"][..],
            &["supervisor", "--no-cortex", "--no-cortex"][..],
            &["supervisor", "--no-spl", "--no-spl"][..],
            &["supervisor", "--no-daily", "--no-daily"][..],
            &["supervisor", "--no-schedule", "--no-schedule"][..],
            &["supervisor", "--journal", "/a", "--journal", "/b"][..],
            &["supervisor", "5015", "6015"][..],
            &["supervisor", "nope-a-port"][..],
            &["supervisor", "--unknown"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::SupervisorUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn supervisor_lifecycle_parse_failures_return_redirect() {
        for verb in [
            "start",
            "stop",
            "restart",
            "status",
            "install",
            "uninstall",
            "logs",
        ] {
            assert_eq!(
                evaluate_args(&args(&["supervisor", verb])),
                Ok(Command::SupervisorLifecycleRedirect(verb)),
                "{verb}"
            );
        }
    }

    #[test]
    fn parses_schedule_arguments_before_execution() {
        assert_eq!(
            SCHEDULE_HELP,
            "usage: journal schedule [-h] [-v] [-d]\n\nShow scheduled tasks\n\noptions:\n  -h, --help     show this help message and exit\n  -v, --verbose  Enable verbose output\n  -d, --debug    Enable debug logging\n"
        );
        assert_eq!(
            evaluate_args(&args(&["schedule"])),
            Ok(Command::Schedule(ScheduleOptions {
                verbose: false,
                debug: false,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["schedule", "-v", "--debug"])),
            Ok(Command::Schedule(ScheduleOptions {
                verbose: true,
                debug: true,
            }))
        );
        assert_eq!(
            evaluate_args(&args(&["schedule", "--help"])),
            Ok(Command::ScheduleHelp)
        );
        assert_eq!(
            evaluate_args(&args(&["schedule", "--nonsense"])),
            Ok(Command::ScheduleUsage(ScheduleUsageError(
                "unrecognized arguments: --nonsense".to_owned()
            )))
        );
    }

    #[test]
    fn parses_sense_batch_options_before_any_runtime_touch() {
        assert_eq!(
            evaluate_args(&args(&[
                "sense",
                "--day",
                "not-a-date",
                "-j",
                "9",
                "--reprocess",
                "screen",
                "--segment",
                "120000_1",
                "--stream",
                "capture.1",
                "--dry-run",
                "-v",
                "-d",
            ])),
            Ok(Command::Sense(SenseOptions {
                day: Some("not-a-date".into()),
                jobs: 9,
                reprocess: Some(SenseReprocessKind::Screen),
                segment: Some("120000_1".into()),
                stream: Some("capture.1".into()),
                dry_run: true,
                verbose: true,
                debug: true,
            }))
        );
    }

    #[test]
    fn sense_event_options_keep_jobs_as_an_inert_parse_value() {
        assert_eq!(
            evaluate_args(&args(&["sense", "--jobs", "99"])),
            Ok(Command::Sense(SenseOptions {
                day: None,
                jobs: 99,
                reprocess: None,
                segment: None,
                stream: None,
                dry_run: false,
                verbose: false,
                debug: false,
            }))
        );
    }

    #[test]
    fn sense_help_and_invalid_combinations_have_verb_owned_outcomes() {
        assert_eq!(
            evaluate_args(&args(&["sense", "--help"])),
            Ok(Command::SenseHelp)
        );
        for values in [
            &["sense", "--nonsense"][..],
            &["sense", "--reprocess", "bogus", "--day", "20260812"][..],
            &["sense", "--segment", "120000_1"][..],
            &["sense", "--stream", "Upper"][..],
            &["sense", "--dry-run"][..],
            &["sense", "--jobs", "not-an-int"][..],
        ] {
            assert_eq!(
                evaluate_args(&args(values)),
                Ok(Command::SenseUsage),
                "{values:?}"
            );
        }
    }

    #[test]
    fn cortex_nonsense_is_detail_carrying_usage_error() {
        assert_eq!(
            evaluate_args(&args(&["cortex", "--nonsense"])),
            Ok(Command::CortexUsage(CortexUsageError(
                "unrecognized arguments: --nonsense".into()
            )))
        );
    }

    #[test]
    fn cortex_help_matches_argparse_body_verbatim() {
        assert_eq!(
            evaluate_args(&args(&["cortex", "--help"])),
            Ok(Command::CortexHelp)
        );
        assert_eq!(
            CORTEX_HELP,
            "usage: journal cortex [-h] [-v] [-d]\n\nsolstone Cortex Talent Manager\n\noptions:\n  -h, --help     show this help message and exit\n  -v, --verbose  Enable verbose output\n  -d, --debug    Enable debug logging\n"
        );
    }

    #[test]
    fn mcp_command_grammar_is_available_without_the_endpoint_feature() {
        assert_eq!(
            evaluate_args(&args(&["mcp", "service"])),
            Ok(Command::Mcp(McpCommand::Service))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "local-door"])),
            Ok(Command::Mcp(McpCommand::LocalDoor))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "status"])),
            Ok(Command::Mcp(McpCommand::Status))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "token", "create", "--label", "operator"])),
            Ok(Command::Mcp(McpCommand::Token(McpTokenCommand::Create {
                label: "operator".to_owned(),
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "token", "list"])),
            Ok(Command::Mcp(McpCommand::Token(McpTokenCommand::List)))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "token", "revoke", "--label", "operator"])),
            Ok(Command::Mcp(McpCommand::Token(McpTokenCommand::Revoke {
                label: "operator".to_owned(),
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "pairing", "generate"])),
            Ok(Command::Mcp(McpCommand::Pairing(
                McpPairingCommand::Generate { door: None }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "pairing", "generate", "--door", "lan"])),
            Ok(Command::Mcp(McpCommand::Pairing(
                McpPairingCommand::Generate {
                    door: Some("lan".to_owned())
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "pairing", "revoke"])),
            Ok(Command::Mcp(McpCommand::Pairing(McpPairingCommand::Revoke)))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "oauth", "list"])),
            Ok(Command::Mcp(McpCommand::Oauth(McpOauthCommand::List)))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "oauth", "revoke", "--client-id", "X"])),
            Ok(Command::Mcp(McpCommand::Oauth(McpOauthCommand::Revoke {
                client_id: "X".to_owned(),
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "permission", "show"])),
            Ok(Command::Mcp(McpCommand::Permission(
                McpPermissionCommand::Show { target: None }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "mcp",
                "permission",
                "show",
                "--token",
                "test-agent"
            ])),
            Ok(Command::Mcp(McpCommand::Permission(
                McpPermissionCommand::Show {
                    target: Some(McpTarget::Token {
                        label: "test-agent".to_owned(),
                    }),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "mcp",
                "permission",
                "set",
                "--oauth",
                "client-1",
                "--created",
                "2026-09-13T18:00:00Z",
                "--category",
                "transcripts",
                "--facet",
                "work"
            ])),
            Ok(Command::Mcp(McpCommand::Permission(
                McpPermissionCommand::Set {
                    target: McpTarget::Oauth {
                        client_id: "client-1".to_owned(),
                        created_at: Some("2026-09-13T18:00:00Z".to_owned()),
                    },
                    categories: vec!["transcripts".to_owned()],
                    facets: vec!["work".to_owned()],
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "mcp",
                "probe",
                "--token",
                "test-agent",
                "--tool",
                "list_facets",
                "--arguments",
                "{}"
            ])),
            Ok(Command::Mcp(McpCommand::Probe(McpProbeCommand {
                target: McpTarget::Token {
                    label: "test-agent".to_owned()
                },
                tool: "list_facets".to_owned(),
                arguments: "{}".to_owned(),
            })))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "mcp",
                "permission",
                "clear",
                "--token",
                "test-agent"
            ])),
            Ok(Command::Mcp(McpCommand::Permission(
                McpPermissionCommand::Clear {
                    target: McpTarget::Token {
                        label: "test-agent".to_owned(),
                    },
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp"])),
            Ok(Command::McpUsage(McpUsageError(
                "the following arguments are required: service, local-door, status, token, pairing, oauth, permission, activity, probe"
                    .to_owned()
            )))
        );
        // The activity grammar parses without the endpoint feature too; only
        // its implementation is gated.
        assert_eq!(
            evaluate_args(&args(&["mcp", "activity", "--outcome", "refused"])),
            Ok(Command::Mcp(McpCommand::Activity(McpActivityCommand {
                target: None,
                tool: None,
                outcome: Some("refused".to_owned()),
                day_from: None,
                day_to: None,
                limit: DEFAULT_ACTIVITY_LIMIT,
                json: false,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "pairing"])),
            Ok(Command::McpUsage(McpUsageError(
                "the following arguments are required: generate, revoke".to_owned()
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "permission"])),
            Ok(Command::McpUsage(McpUsageError(
                "the following arguments are required: show, set, clear".to_owned()
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "permission", "set"])),
            Ok(Command::McpUsage(McpUsageError(
                "one of --token or --oauth is required".to_owned()
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "oauth"])),
            Ok(Command::McpUsage(McpUsageError(
                "the following arguments are required: list, revoke".to_owned()
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "oauth", "revoke"])),
            Ok(Command::McpUsage(McpUsageError(
                "the following arguments are required: --client-id CLIENT_ID".to_owned()
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "oauth", "revoke", "--client-id", ""])),
            Ok(Command::McpUsage(McpUsageError(
                "expected exactly one --client-id CLIENT_ID argument".to_owned()
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["mcp", "--help"])),
            Ok(Command::McpHelp)
        );
    }

    #[test]
    fn backfill_facet_ids_grammar_tests() {
        assert_eq!(
            evaluate_args(&args(&["backfill-facet-ids"])),
            Ok(Command::BackfillFacetIds { commit: false })
        );
        assert_eq!(
            evaluate_args(&args(&["backfill-facet-ids", "--commit"])),
            Ok(Command::BackfillFacetIds { commit: true })
        );
        assert_eq!(
            evaluate_args(&args(&["backfill-facet-ids", "--help"])),
            Ok(Command::BackfillFacetIdsHelp)
        );
        assert_eq!(
            evaluate_args(&args(&["backfill-facet-ids", "--nonsense"])),
            Ok(Command::BackfillFacetIdsUsage)
        );
    }

    #[test]
    fn journal_route_inspect_is_hidden_and_accepts_no_arguments() {
        assert_eq!(
            evaluate_args(&args(&["__journal-route-inspect"])),
            Ok(Command::JournalRouteInspect)
        );
        assert!(evaluate_args(&args(&["__journal-route-inspect", "--help"])).is_err());
    }

    #[test]
    fn journal_route_repair_requires_one_lowercase_lock_owner() {
        assert_eq!(
            evaluate_args(&args(&[
                "__journal-route-repair",
                "--route-lock-owner",
                "0123456789abcdef0123456789abcdef",
            ])),
            Ok(Command::JournalRouteRepair {
                lock_owner: "0123456789abcdef0123456789abcdef".to_owned(),
            })
        );
        for invalid in [
            args(&["__journal-route-repair"]),
            args(&[
                "__journal-route-repair",
                "--route-lock-owner",
                "0123456789ABCDEF0123456789ABCDEF",
            ]),
            args(&[
                "__journal-route-repair",
                "--route-lock-owner",
                "0123456789abcdef0123456789abcdef",
                "extra",
            ]),
        ] {
            assert!(evaluate_args(&invalid).is_err());
        }
    }
}
