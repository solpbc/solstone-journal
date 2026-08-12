// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Spawned-binary reachability coverage for the native journal importer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use solstone_core_segment::SUPERVISOR_MESSAGE;
use tempfile::TempDir;

const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");
const ORACLE: &str = include_str!("../../../fixtures/import_cli_help_oracle.json");
const TOP_LEVEL_USAGE: &str = "solstone-core --version";

#[derive(Clone, Copy)]
enum Input {
    Audio,
    Ics,
    Vault,
    Pdf,
    Image,
    Archive,
    AppleExport,
    Oura,
    AudioDirectory,
    Kindle,
}

#[derive(Clone, Copy)]
enum Invocation {
    GenericAudio,
    Structured { source: &'static str, input: Input },
    ListImporters,
    ListImportersJson,
    Backends,
    SyncAudio,
    ConnectUnknown,
    JournalSourceCreate,
    JournalSourceList,
    JournalSourceStatus,
    JournalSourceRevoke,
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

struct Expected {
    exit: i32,
    stream: Stream,
    identifies: &'static str,
}

struct ModeCase {
    name: &'static str,
    invocations: &'static [(Invocation, Expected)],
}

const MODE_CASES: &[ModeCase] = &[
    ModeCase {
        name: "generic media",
        invocations: &[(
            Invocation::GenericAudio,
            Expected {
                exit: 0,
                stream: Stream::Stdout,
                identifies: "day: \"20260311\"",
            },
        )],
    },
    ModeCase {
        name: "structured sources",
        invocations: &[
            (
                Invocation::Structured {
                    source: "ics",
                    input: Input::Ics,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Import preview ready: ics",
                },
            ),
            (
                Invocation::Structured {
                    source: "obsidian",
                    input: Input::Vault,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Import preview ready: obsidian",
                },
            ),
            (
                Invocation::Structured {
                    source: "document",
                    input: Input::Pdf,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Import preview ready: document",
                },
            ),
            (
                Invocation::Structured {
                    source: "image",
                    input: Input::Image,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Import preview ready: image",
                },
            ),
            (
                Invocation::Structured {
                    source: "journal_archive",
                    input: Input::Archive,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Import preview ready: journal_archive",
                },
            ),
            (
                Invocation::Structured {
                    source: "chatgpt",
                    input: Input::Archive,
                },
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "import-sources: unimplemented: chatgpt",
                },
            ),
            (
                Invocation::Structured {
                    source: "claude",
                    input: Input::Archive,
                },
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "import-sources: unimplemented: claude",
                },
            ),
            (
                Invocation::Structured {
                    source: "gemini",
                    input: Input::Archive,
                },
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "import-sources: unimplemented: gemini",
                },
            ),
            (
                Invocation::Structured {
                    source: "kindle",
                    input: Input::Kindle,
                },
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "import-sources: unimplemented: kindle",
                },
            ),
        ],
    },
    ModeCase {
        name: "apple native return",
        invocations: &[(
            Invocation::Structured {
                source: "apple_health",
                input: Input::AppleExport,
            },
            Expected {
                exit: 0,
                stream: Stream::Stdout,
                identifies: "\"source\":\"apple_health\"",
            },
        )],
    },
    ModeCase {
        name: "oura file refusal",
        invocations: &[(
            Invocation::Structured {
                source: "oura",
                input: Input::Oura,
            },
            Expected {
                exit: 1,
                stream: Stream::Stderr,
                identifies: "Oura body data imports through sync; use journal importer --sync oura",
            },
        )],
    },
    ModeCase {
        name: "importer listing",
        invocations: &[
            (
                Invocation::ListImporters,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "File importers:",
                },
            ),
            (
                Invocation::ListImportersJson,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "\"journal_archive\"",
                },
            ),
        ],
    },
    ModeCase {
        name: "backends",
        invocations: &[(
            Invocation::Backends,
            Expected {
                exit: 0,
                stream: Stream::Stdout,
                identifies: "Syncable backends:",
            },
        )],
    },
    ModeCase {
        name: "sync",
        invocations: &[(
            Invocation::SyncAudio,
            Expected {
                exit: 0,
                stream: Stream::Stdout,
                identifies: "Syncing audio (catalog mode)...",
            },
        )],
    },
    ModeCase {
        name: "connect",
        invocations: &[(
            Invocation::ConnectUnknown,
            Expected {
                exit: 1,
                stream: Stream::Stderr,
                identifies: "Unknown connect backend: unknown",
            },
        )],
    },
    ModeCase {
        name: "journal-source",
        invocations: &[
            (
                Invocation::JournalSourceCreate,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Journal source created:",
                },
            ),
            (
                Invocation::JournalSourceList,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "w11-source",
                },
            ),
            (
                Invocation::JournalSourceStatus,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Journal source: w11-source",
                },
            ),
            (
                Invocation::JournalSourceRevoke,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Revoked journal source 'w11-source'",
                },
            ),
        ],
    },
];

struct Inputs {
    audio: PathBuf,
    ics: PathBuf,
    vault: PathBuf,
    pdf: PathBuf,
    image: PathBuf,
    archive: PathBuf,
    apple_export: PathBuf,
    oura: PathBuf,
    audio_directory: PathBuf,
    kindle: PathBuf,
}

impl Inputs {
    fn create(journal: &TempDir) -> Self {
        let directory = journal.path().join("inputs");
        fs::create_dir_all(&directory).expect("input directory");
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let audio = directory.join("audio.m4a");
        fs::copy(
            root.join("tests/fixtures/audio/aac_single_track.m4a"),
            &audio,
        )
        .expect("copy audio fixture");
        let ics = directory.join("calendar.ics");
        fs::write(&ics, "BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n").expect("ICS input");
        let vault = directory.join("vault");
        fs::create_dir(&vault).expect("vault directory");
        fs::write(vault.join("note.md"), "# Imported note\n").expect("vault note");
        let pdf = directory.join("document.pdf");
        fs::copy(root.join("core/fixtures/pdf_corpus/text.pdf"), &pdf).expect("copy PDF fixture");
        let image = directory.join("image.png");
        image::RgbaImage::new(1, 1)
            .save(&image)
            .expect("image input");
        let archive = directory.join("archive.zip");
        fs::copy(
            root.join("tests/fixtures/importers/health/apple_health_synthetic.zip"),
            &archive,
        )
        .expect("copy archive fixture");
        let apple_export = directory.join("apple_health.zip");
        fs::copy(&archive, &apple_export).expect("copy Apple export");
        let oura = directory.join("daily_sleep.json");
        fs::copy(
            root.join("core/fixtures/body-source/inputs/oura/daily_sleep.json"),
            &oura,
        )
        .expect("copy Oura input");
        let audio_directory = directory.join("audio-directory");
        fs::create_dir(&audio_directory).expect("audio directory");
        fs::copy(&audio, audio_directory.join("audio.m4a")).expect("copy sync audio");
        let kindle = directory.join("My Clippings.txt");
        fs::write(
            &kindle,
            "Book title\n- Your Highlight\nLocation 1\n\nText\n==========\n",
        )
        .expect("Kindle input");
        Self {
            audio,
            ics,
            vault,
            pdf,
            image,
            archive,
            apple_export,
            oura,
            audio_directory,
            kindle,
        }
    }

    fn path(&self, input: Input) -> &Path {
        match input {
            Input::Audio => &self.audio,
            Input::Ics => &self.ics,
            Input::Vault => &self.vault,
            Input::Pdf => &self.pdf,
            Input::Image => &self.image,
            Input::Archive => &self.archive,
            Input::AppleExport => &self.apple_export,
            Input::Oura => &self.oura,
            Input::AudioDirectory => &self.audio_directory,
            Input::Kindle => &self.kindle,
        }
    }
}

impl Invocation {
    fn args(self, inputs: &Inputs) -> Vec<String> {
        match self {
            Self::GenericAudio => vec![
                path(inputs.path(Input::Audio)),
                "20260311_120000".to_owned(),
            ],
            Self::Structured { source, input } => {
                let mut args = vec![
                    "--source".to_owned(),
                    source.to_owned(),
                    path(inputs.path(input)),
                ];
                if source == "apple_health" {
                    args.extend(["--dry-run".to_owned(), "--json".to_owned()]);
                }
                args
            }
            Self::ListImporters => vec!["--list-importers".to_owned()],
            Self::ListImportersJson => vec!["--list-importers".to_owned(), "--json".to_owned()],
            Self::Backends => vec!["--backends".to_owned()],
            Self::SyncAudio => vec![
                "--sync".to_owned(),
                "audio".to_owned(),
                "--path".to_owned(),
                path(inputs.path(Input::AudioDirectory)),
            ],
            Self::ConnectUnknown => vec!["--connect".to_owned(), "unknown".to_owned()],
            Self::JournalSourceCreate => vec![
                "journal-source".to_owned(),
                "create".to_owned(),
                "w11-source".to_owned(),
            ],
            Self::JournalSourceList => vec!["journal-source".to_owned(), "list".to_owned()],
            Self::JournalSourceStatus => vec![
                "journal-source".to_owned(),
                "status".to_owned(),
                "w11-source".to_owned(),
            ],
            Self::JournalSourceRevoke => vec![
                "journal-source".to_owned(),
                "revoke".to_owned(),
                "w11-source".to_owned(),
            ],
        }
    }
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str], journal: &TempDir) -> Output {
    run_owned(
        &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        journal,
    )
}

fn run_owned(args: &[String], journal: &TempDir) -> Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .arg("importer")
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env_remove("SOL_SUPERVISOR_SPAWNED")
        .output()
        .expect("run importer")
}

fn run_down(args: &[String], journal: &TempDir) -> Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .arg("importer")
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
        .env_remove("SOL_SUPERVISOR_SPAWNED")
        .output()
        .expect("run importer")
}

fn run_case(
    case: &ModeCase,
    journal: &TempDir,
    inputs: &Inputs,
) -> Vec<(&'static Expected, Output)> {
    case.invocations
        .iter()
        .map(|(invocation, expected)| (expected, run_owned(&invocation.args(inputs), journal)))
        .collect()
}

#[test]
fn every_mode_has_its_promised_observable() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    for case in MODE_CASES {
        for (expected, output) in run_case(case, &journal, &inputs) {
            assert_eq!(output.status.code(), Some(expected.exit), "{}", case.name);
            let payload = match expected.stream {
                Stream::Stdout => String::from_utf8_lossy(&output.stdout),
                Stream::Stderr => String::from_utf8_lossy(&output.stderr),
            };
            assert!(
                payload.contains(expected.identifies),
                "{}: {payload}",
                case.name
            );
        }
    }
}

#[test]
fn every_mode_is_reachable_past_the_top_level_dispatch() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    let mut failures = Vec::new();
    for case in MODE_CASES {
        let outputs = run_case(case, &journal, &inputs);
        if let Some((_, output)) = outputs.iter().find(|(_, output)| {
            output.status.code() == Some(64)
                || String::from_utf8_lossy(&output.stderr).contains(TOP_LEVEL_USAGE)
        }) {
            failures.push(format!(
                "=== {} ===\nexit={:?}\nstdout:\n{}stderr:\n{}",
                case.name,
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn list_importers_json_uses_the_grammar_fixture_as_its_oracle() {
    let journal = TempDir::new().expect("journal");
    let output = run(&["--list-importers", "--json"], &journal);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let actual: Value = serde_json::from_slice(&output.stdout).expect("importer JSON");
    let grammar: Value = serde_json::from_str(GRAMMAR).expect("grammar fixture");
    assert_eq!(actual, grammar["importers"]);
}

#[test]
fn list_and_backend_human_forms_match_the_usable_oracle_cases() {
    let journal = TempDir::new().expect("journal");
    let oracle: Value = serde_json::from_str(ORACLE).expect("help oracle");
    for (args, case) in [
        (&["--list-importers"][..], "gate_passed/list_importers"),
        (&["--backends"][..], "gate_passed/backends"),
    ] {
        let output = run(args, &journal);
        assert_eq!(output.status.code(), Some(0), "{case}");
        assert!(output.stderr.is_empty(), "{case}");
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout"),
            oracle["cases"][case]["stdout"]
                .as_str()
                .expect("oracle stdout"),
            "{case}"
        );
    }
}

#[test]
#[ignore = "help_long, help_short, and journal_source_help await a corrected help-oracle capture; see docs/design/import-reachability-w11.md"]
fn help_fidelity_is_fixture_exact_when_the_capture_is_corrected() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("help oracle");
    for column in ["gate_passed", "solstone_down"] {
        for case in ["help_long", "help_short", "journal_source_help"] {
            let fixture_case = format!("{column}/{case}");
            let expected = &oracle["cases"][&fixture_case];
            let args = expected["argv"]
                .as_array()
                .expect("fixture argv")
                .iter()
                .map(|value| value.as_str().expect("fixture argv string").to_owned())
                .collect::<Vec<_>>();
            let journal = TempDir::new().expect("journal");
            let output = if column == "gate_passed" {
                run_owned(&args, &journal)
            } else {
                run_down(&args, &journal)
            };
            assert_eq!(
                output.status.code(),
                expected["exit"].as_i64().map(|code| code as i32),
                "{fixture_case}"
            );
            assert_eq!(
                output.stdout,
                expected["stdout"]
                    .as_str()
                    .expect("fixture stdout")
                    .as_bytes(),
                "{fixture_case}"
            );
            assert_eq!(
                output.stderr,
                expected["stderr"]
                    .as_str()
                    .expect("fixture stderr")
                    .as_bytes(),
                "{fixture_case}"
            );
        }
    }
}

#[test]
fn down_supervisor_reaches_the_shared_unavailable_refusal() {
    let journal = TempDir::new().expect("journal");
    let output = run_down(&["--list-importers".to_owned()], &journal);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, format!("{SUPERVISOR_MESSAGE}\n").as_bytes());
}

#[test]
fn down_supervisor_unknown_option_is_a_parse_error() {
    let journal = TempDir::new().expect("journal");
    let output = run_down(&["--nonsense".to_owned()], &journal);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized arguments: --nonsense"));
}

#[test]
fn gate_passed_unknown_option_is_a_parse_error() {
    let journal = TempDir::new().expect("journal");
    let output = run(&["--nonsense"], &journal);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized arguments: --nonsense"));
}

#[test]
fn surplus_positionals_are_rejected_loudly() {
    let journal = TempDir::new().expect("journal");
    let output = run(&["a.m4a", "20260311_120000", "b.m4a"], &journal);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized arguments: b.m4a"));
}

#[test]
fn spawned_unavailable_importer_is_silent() {
    let journal = TempDir::new().expect("journal");
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["importer", "file"])
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOL_SUPERVISOR_SPAWNED", "1")
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
        .output()
        .expect("run importer");
    assert_eq!(output.status.code(), Some(75));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
