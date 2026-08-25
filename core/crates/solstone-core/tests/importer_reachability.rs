// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Spawned-binary reachability coverage for the native journal importer.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use solstone_core_segment::SUPERVISOR_MESSAGE;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");
const ORACLE: &str = include_str!("../../../fixtures/import_cli_help_oracle.json");
const TOP_LEVEL_USAGE: &str = "solstone-core --version";

#[derive(Clone, Copy)]
enum Input {
    Audio,
    Text,
    Ics,
    Vault,
    Pdf,
    Image,
    Archive,
    Claude,
    Chatgpt,
    Gemini,
    AppleExport,
    Oura,
    AudioDirectory,
    Kindle,
}

#[derive(Clone, Copy)]
enum Invocation {
    GenericAudio,
    GenericAudioDryRun,
    GenericText,
    GenericAutoTimestamp,
    UnclassifiedMedia,
    Structured { source: &'static str, input: Input },
    ListImporters,
    ListImportersJson,
    Backends,
    SyncAudio,
    SyncObsidian,
    SyncPlaudSave,
    ConnectUnknown,
    JournalSourceCreate,
    JournalSourceList,
    JournalSourceStatus,
    JournalSourceRevoke,
}

impl Invocation {
    fn writes_journal_state(self) -> bool {
        matches!(
            self,
            Self::Structured {
                source: "document" | "image" | "journal_archive",
                ..
            }
        )
    }
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
        invocations: &[
            (
                Invocation::GenericAudio,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "day: \"20260311\"",
                },
            ),
            (
                Invocation::GenericText,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Generic text import complete",
                },
            ),
            (
                Invocation::GenericAudioDryRun,
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "generic audio preview requires the audio import body's preview path",
                },
            ),
            (
                Invocation::GenericAutoTimestamp,
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "detected timestamp",
                },
            ),
            (
                Invocation::UnclassifiedMedia,
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "automatic source classification requires solstone-core-import-sources registry claims",
                },
            ),
        ],
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
                    identifies: "2 events, 1 unique attendees",
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
                    identifies: "1 daily notes, 2 knowledge notes, 1 unique wikilinks",
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
                    identifies: "document import complete: entries_written=1",
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
                    identifies: "image import complete: entries_written=1",
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
                    identifies: "journal_archive import complete: segments_copied=1",
                },
            ),
            (
                Invocation::Structured {
                    source: "chatgpt",
                    input: Input::Chatgpt,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "1 messages from ChatGPT export",
                },
            ),
            (
                Invocation::Structured {
                    source: "claude",
                    input: Input::Claude,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "2 messages from Claude chat export",
                },
            ),
            (
                Invocation::Structured {
                    source: "gemini",
                    input: Input::Gemini,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "1 messages from Gemini export",
                },
            ),
            (
                Invocation::Structured {
                    source: "kindle",
                    input: Input::Kindle,
                },
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "1 highlights from 1 books",
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
        invocations: &[
            (
                Invocation::SyncAudio,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Audio sync preview complete: source=",
                },
            ),
            (
                Invocation::SyncObsidian,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Obsidian sync preview complete: source=",
                },
            ),
            (
                Invocation::SyncPlaudSave,
                Expected {
                    exit: 1,
                    stream: Stream::Stderr,
                    identifies: "Plaud sync save requires native credential, download, and import pipeline adapters",
                },
            ),
        ],
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
                    identifies: "sample-source",
                },
            ),
            (
                Invocation::JournalSourceStatus,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Journal source: sample-source",
                },
            ),
            (
                Invocation::JournalSourceRevoke,
                Expected {
                    exit: 0,
                    stream: Stream::Stdout,
                    identifies: "Revoked journal source 'sample-source'",
                },
            ),
        ],
    },
];

struct Inputs {
    audio: PathBuf,
    text: PathBuf,
    ics: PathBuf,
    vault: PathBuf,
    pdf: PathBuf,
    image: PathBuf,
    archive: PathBuf,
    claude: PathBuf,
    chatgpt: PathBuf,
    gemini: PathBuf,
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
        let text = directory.join("transcript.txt");
        fs::write(&text, "A short imported transcript.").expect("text input");
        let ics = directory.join("calendar.ics");
        fs::write(&ics, "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nDTSTART:20260311T120000Z\r\nCREATED:20260311T120000Z\r\nATTENDEE;CN=Taylor:mailto:taylor@example.com\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20260312T120000Z\r\nCREATED:20260312T120000Z\r\nATTENDEE;CN=Taylor:mailto:taylor@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").expect("ICS input");
        let vault = directory.join("vault");
        fs::create_dir(&vault).expect("vault directory");
        fs::create_dir(vault.join(".obsidian")).expect("vault marker");
        fs::write(vault.join("2026-03-11.md"), "# Daily\n[[Topic]]\n").expect("daily note");
        fs::write(vault.join("Topic.md"), "# Topic\n").expect("topic note");
        fs::write(vault.join("Other.md"), "# Other\n").expect("other note");
        let pdf = directory.join("document.pdf");
        fs::copy(root.join("core/fixtures/pdf_corpus/text.pdf"), &pdf).expect("copy PDF fixture");
        let image = directory.join("image.png");
        image::RgbaImage::new(1, 1)
            .save(&image)
            .expect("image input");
        let archive = directory.join("archive.zip");
        write_zip(
            &archive,
            &[(
                "archive/chronicle/20260311/120000_60/value",
                b"archive import",
            )],
        );
        let claude = directory.join("claude.zip");
        write_zip(&claude, &[("conversations.json", br#"[{"created_at":"2026-03-11T12:00:00","chat_messages":[{"sender":"human","text":"Hello","created_at":"2026-03-11T12:00:00"},{"sender":"assistant","text":"Hi","created_at":"2026-03-11T12:01:00"}]}]"#)]);
        let chatgpt = directory.join("chatgpt.zip");
        write_zip(&chatgpt, &[("conversations.json", br#"[{"mapping":{"root":{"message":{"author":{"role":"user"},"content":{"parts":["Hello"]},"create_time":1773230400.0},"parent":null}},"current_node":"root"}]"#)]);
        let gemini = directory.join("gemini.zip");
        write_zip(&gemini, &[("Takeout/My Activity/Gemini Apps/MyActivity.json", br#"[{"time":"2026-03-11T12:00:00Z","subtitles":[{"value":"prompt"}],"products":["Gemini"],"header":"Gemini"}]"#)]);
        let apple_export = directory.join("apple_health.zip");
        fs::copy(
            root.join("tests/fixtures/importers/health/apple_health_synthetic.zip"),
            &apple_export,
        )
        .expect("copy Apple export");
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
        fs::write(&kindle, "A Book (An Author)\n- Your Highlight on page 1 | Added on Wednesday, March 11, 2026 12:00:00 PM\n\nA highlight\n==========\n")
        .expect("Kindle input");
        Self {
            audio,
            text,
            ics,
            vault,
            pdf,
            image,
            archive,
            claude,
            chatgpt,
            gemini,
            apple_export,
            oura,
            audio_directory,
            kindle,
        }
    }

    fn path(&self, input: Input) -> &Path {
        match input {
            Input::Audio => &self.audio,
            Input::Text => &self.text,
            Input::Ics => &self.ics,
            Input::Vault => &self.vault,
            Input::Pdf => &self.pdf,
            Input::Image => &self.image,
            Input::Archive => &self.archive,
            Input::Claude => &self.claude,
            Input::Chatgpt => &self.chatgpt,
            Input::Gemini => &self.gemini,
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
            Self::GenericAudioDryRun => vec![
                "--dry-run".to_owned(),
                path(inputs.path(Input::Audio)),
                "20260311_120000".to_owned(),
            ],
            Self::GenericText => vec![path(inputs.path(Input::Text)), "20260311_120000".to_owned()],
            Self::GenericAutoTimestamp => vec![path(inputs.path(Input::Text))],
            Self::UnclassifiedMedia => vec![
                "--timestamp".to_owned(),
                "20260311_120000".to_owned(),
                path(inputs.path(Input::Image)),
            ],
            Self::Structured { source, input } => {
                let mut args = vec![
                    "--source".to_owned(),
                    source.to_owned(),
                    "--timestamp".to_owned(),
                    "20260311_120000".to_owned(),
                    path(inputs.path(input)),
                ];
                if matches!(
                    source,
                    "ics" | "obsidian" | "claude" | "chatgpt" | "kindle" | "gemini"
                ) {
                    args.push("--dry-run".to_owned());
                }
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
            Self::SyncObsidian => vec![
                "--sync".to_owned(),
                "obsidian".to_owned(),
                "--path".to_owned(),
                path(inputs.path(Input::Vault)),
            ],
            Self::SyncPlaudSave => {
                vec!["--sync".to_owned(), "plaud".to_owned(), "--save".to_owned()]
            }
            Self::ConnectUnknown => vec!["--connect".to_owned(), "unknown".to_owned()],
            Self::JournalSourceCreate => vec![
                "journal-source".to_owned(),
                "create".to_owned(),
                "sample-source".to_owned(),
            ],
            Self::JournalSourceList => vec!["journal-source".to_owned(), "list".to_owned()],
            Self::JournalSourceStatus => vec![
                "journal-source".to_owned(),
                "status".to_owned(),
                "sample-source".to_owned(),
            ],
            Self::JournalSourceRevoke => vec![
                "journal-source".to_owned(),
                "revoke".to_owned(),
                "sample-source".to_owned(),
            ],
        }
    }
}

fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
    let file = File::create(path).expect("create zip");
    let mut writer = zip::ZipWriter::new(file);
    for (name, bytes) in members {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .expect("zip member");
        writer.write_all(bytes).expect("zip content");
    }
    writer.finish().expect("finish zip");
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str], journal: &TempDir) -> Output {
    run_in_column(
        SupervisorColumn::GatePassed,
        &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        journal,
    )
}

#[derive(Clone, Copy)]
enum SupervisorColumn {
    GatePassed,
    SolstoneDown,
    SpawnedUnavailable,
}

impl SupervisorColumn {
    fn fixture_name(self) -> &'static str {
        match self {
            Self::GatePassed => "gate_passed",
            Self::SolstoneDown => "solstone_down",
            Self::SpawnedUnavailable => "supervisor_spawned",
        }
    }

    fn configure(self, command: &mut Command) {
        match self {
            Self::GatePassed => {
                command
                    .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
                    .env_remove("SOL_SUPERVISOR_SPAWNED");
            }
            Self::SolstoneDown => {
                command
                    .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
                    .env_remove("SOL_SUPERVISOR_SPAWNED");
            }
            Self::SpawnedUnavailable => {
                command
                    .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
                    .env("SOL_SUPERVISOR_SPAWNED", "1");
            }
        }
    }
}

fn run_in_column(column: SupervisorColumn, args: &[String], journal: &TempDir) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .arg("importer")
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOLSTONE_CORE_PDF_LIBRARY", pdfium_library());
    column.configure(&mut command);
    command.output().expect("run importer")
}

fn pdfium_library() -> PathBuf {
    let (target, filename) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => ("linux-x86_64", "libpdfium.so"),
        ("linux", "aarch64") => ("linux-aarch64", "libpdfium.so"),
        ("macos", "aarch64") => ("macos-arm64", "libpdfium.dylib"),
        (os, arch) => panic!("unsupported PDFium test host: {os}/{arch}"),
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("target/pdfium-runtime-link")
        .join(target)
        .join(filename);
    assert!(
        path.is_file(),
        "PDFium runtime is not staged; run make check-rust-pdf-stage"
    );
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core"))
        .parent()
        .expect("core binary parent")
        .join("solstone-core-pdf");
    assert!(
        worker.is_file(),
        "PDF worker is not built; run cargo build --manifest-path core/Cargo.toml -p solstone-core-pdf --locked"
    );
    path
}

fn run_case(
    case: &ModeCase,
    journal: &TempDir,
    inputs: &Inputs,
) -> Vec<(&'static Expected, Output)> {
    case.invocations
        .iter()
        .map(|(invocation, expected)| {
            let isolated_journal = invocation
                .writes_journal_state()
                .then(|| TempDir::new().expect("pristine journal"));
            let target_journal = isolated_journal.as_ref().unwrap_or(journal);
            let _processing = matches!(invocation, Invocation::GenericAudio)
                .then(|| AudioProcessingCompleter::start(target_journal.path()));
            let _rescan = matches!(
                invocation,
                Invocation::Structured {
                    source: "journal_archive",
                    ..
                }
            )
            .then(|| swallow_rescans(target_journal.path()));
            (
                expected,
                run_in_column(
                    SupervisorColumn::GatePassed,
                    &invocation.args(inputs),
                    target_journal,
                ),
            )
        })
        .collect()
}

#[test]
fn every_mode_has_its_promised_observable_and_reaches_dispatch() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    let mut top_level_failures = Vec::new();
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
            let quiet = match expected.stream {
                Stream::Stdout => &output.stderr,
                Stream::Stderr => &output.stdout,
            };
            assert!(quiet.is_empty(), "{} wrote to both streams", case.name);
            if output.status.code() == Some(64)
                || String::from_utf8_lossy(&output.stderr).contains(TOP_LEVEL_USAGE)
            {
                top_level_failures.push(format!(
                    "=== {} ===\nexit={:?}\nstdout:\n{}stderr:\n{}",
                    case.name,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ));
            }
        }
    }
    assert!(
        top_level_failures.is_empty(),
        "{}",
        top_level_failures.join("\n")
    );
}

fn swallow_rescans(journal: &Path) -> thread::JoinHandle<()> {
    let health = journal.join("health");
    fs::create_dir_all(&health).expect("health");
    let listener = UnixListener::bind(health.join("callosum.sock")).expect("bind callosum");
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut line = String::new();
            let _ = stream.read_to_string(&mut line);
        }
    })
}

#[test]
fn journal_archive_remerge_reports_already_present() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    let _rescans = swallow_rescans(journal.path());
    let args = Invocation::Structured {
        source: "journal_archive",
        input: Input::Archive,
    }
    .args(&inputs);

    let first = run_in_column(SupervisorColumn::GatePassed, &args, &journal);
    assert_eq!(first.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&first.stdout).contains("segments_copied=1"));
    assert!(first.stderr.is_empty());

    let second = run_in_column(SupervisorColumn::GatePassed, &args, &journal);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(
        second.stdout,
        b"journal_archive import did not merge anything: archive content is already present\n"
    );
    assert!(second.stderr.is_empty());
}

#[test]
fn journal_archive_import_requests_supervisor_indexer_rescan() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    let health = journal.path().join("health");
    fs::create_dir_all(&health).expect("health");
    let listener = UnixListener::bind(health.join("callosum.sock")).expect("bind callosum");
    let receiver = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept rescan");
        let mut line = String::new();
        stream.read_to_string(&mut line).expect("read envelope");
        line
    });
    let args = Invocation::Structured {
        source: "journal_archive",
        input: Input::Archive,
    }
    .args(&inputs);
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .arg("importer")
        .args(&args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env_remove("SOL_SUPERVISOR_SPAWNED")
        .output()
        .expect("run journal_archive importer");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = receiver.join().expect("receiver");
    let envelope: Value = serde_json::from_str(line.trim()).expect("rescan json");
    assert_eq!(envelope["tract"], "supervisor");
    assert_eq!(envelope["event"], "request");
    assert_eq!(
        envelope["cmd"],
        serde_json::json!(["journal", "indexer", "--rescan"])
    );
}

#[test]
fn document_import_writes_the_source_and_transcript() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    let output = run_in_column(
        SupervisorColumn::GatePassed,
        &Invocation::Structured {
            source: "document",
            input: Input::Pdf,
        }
        .args(&inputs),
        &journal,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let chronicle = journal.path().join("chronicle");
    let original = find_named_file(&chronicle, "original.pdf").expect("installed PDF original");
    let transcript = find_named_file(&chronicle, "document_transcript.md")
        .expect("installed document transcript");
    let segment = original.parent().expect("original segment");
    assert_eq!(transcript.parent(), Some(segment));
    assert_eq!(
        segment.parent().and_then(Path::file_name),
        Some("import.document".as_ref())
    );
    assert!(original.starts_with(&chronicle));
    assert!(transcript.starts_with(&chronicle));
}

#[test]
fn preview_only_sources_refuse_to_claim_a_write() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    for (source, input) in [
        ("ics", Input::Ics),
        ("obsidian", Input::Vault),
        ("claude", Input::Claude),
        ("chatgpt", Input::Chatgpt),
        ("kindle", Input::Kindle),
        ("gemini", Input::Gemini),
    ] {
        let output = run_in_column(
            SupervisorColumn::GatePassed,
            &[
                "--source".to_owned(),
                source.to_owned(),
                "--timestamp".to_owned(),
                "20260311_120000".to_owned(),
                path(inputs.path(input)),
            ],
            &journal,
        );
        assert_eq!(output.status.code(), Some(1), "{source}");
        assert!(output.stdout.is_empty(), "{source} wrote stdout");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("{source} import previews only and writes nothing; rerun with --dry-run\n")
        );
    }
}

fn find_named_file(root: &Path, name: &str) -> Option<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return None;
    };
    entries.filter_map(Result::ok).find_map(|entry| {
        let path = entry.path();
        if path.file_name().is_some_and(|file_name| file_name == name) {
            Some(path)
        } else if path.is_dir() {
            find_named_file(&path, name)
        } else {
            None
        }
    })
}

struct AudioProcessingCompleter {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl AudioProcessingCompleter {
    fn start(journal: &Path) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let chronicle = journal.join("chronicle");
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(35);
            while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                if let Some(audio) = find_named_file(&chronicle, "imported_audio.m4a") {
                    let input_size = fs::metadata(&audio).expect("imported audio metadata").len();
                    fs::write(
                        audio.with_extension("jsonl"),
                        format!(
                            "{{\"_solstone_processing\":{{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"handler\":\"transcribe\",\"input_size\":{input_size}}}}}\n"
                        ),
                    )
                    .expect("write completed processing record");
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for AudioProcessingCompleter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker
            .take()
            .expect("processing completer worker")
            .join()
            .expect("processing completer worker panicked");
    }
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
#[ignore = "help_long, help_short, and journal_source_help await a corrected help-oracle capture; see docs/design/importer-command-reachability.md"]
fn help_fidelity_is_fixture_exact_when_the_capture_is_corrected() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("help oracle");
    for column in [SupervisorColumn::GatePassed, SupervisorColumn::SolstoneDown] {
        for case in ["help_long", "help_short", "journal_source_help"] {
            let fixture_case = format!("{}/{case}", column.fixture_name());
            let expected = &oracle["cases"][&fixture_case];
            let args = expected["argv"]
                .as_array()
                .expect("fixture argv")
                .iter()
                .map(|value| value.as_str().expect("fixture argv string").to_owned())
                .collect::<Vec<_>>();
            let journal = TempDir::new().expect("journal");
            let output = run_in_column(column, &args, &journal);
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
    let output = run_in_column(
        SupervisorColumn::SolstoneDown,
        &["--list-importers".to_owned()],
        &journal,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, format!("{SUPERVISOR_MESSAGE}\n").as_bytes());
}

#[test]
fn down_supervisor_unknown_option_is_a_parse_error() {
    let journal = TempDir::new().expect("journal");
    let output = run_in_column(
        SupervisorColumn::SolstoneDown,
        &["--nonsense".to_owned()],
        &journal,
    );
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
fn generic_force_bypasses_the_wired_manifest_deduplication() {
    let journal = TempDir::new().expect("journal");
    let inputs = Inputs::create(&journal);
    let source_hash = solstone_core_import::hash_source(inputs.path(Input::Audio))
        .expect("hash audio")
        .into_inner();
    let import = journal.path().join("imports/existing");
    fs::create_dir_all(&import).expect("import directory");
    fs::write(
        import.join("manifest.json"),
        format!(r#"{{"source_hash":"{source_hash}","entry_count":1}}"#),
    )
    .expect("manifest");

    let args = Invocation::GenericAudio.args(&inputs);
    let skipped = run_in_column(SupervisorColumn::GatePassed, &args, &journal);
    assert_eq!(skipped.status.code(), Some(0));
    assert_eq!(skipped.stdout, b"Import skipped: AlreadyImported\n");
    assert!(skipped.stderr.is_empty());

    let mut forced = vec!["--force".to_owned()];
    forced.extend(args);
    let _processing = AudioProcessingCompleter::start(journal.path());
    let imported = run_in_column(SupervisorColumn::GatePassed, &forced, &journal);
    assert_eq!(imported.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&imported.stdout).contains("Generic audio import complete:"));
    assert!(imported.stderr.is_empty());
}

#[test]
fn spawned_unavailable_importer_is_silent() {
    let journal = TempDir::new().expect("journal");
    let output = run_in_column(
        SupervisorColumn::SpawnedUnavailable,
        &["file".to_owned()],
        &journal,
    );
    assert_eq!(output.status.code(), Some(75));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
