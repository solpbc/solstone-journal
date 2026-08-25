// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use zip::write::SimpleFileOptions;

const POISON_INTERPRETER: &str = "#!/bin/sh\nprintf '%s\\n' \"$0\" > \"$POISON_MARKER\"\nexit 97\n";
static SOLSTONE_CORE_BINARY: OnceLock<PathBuf> = OnceLock::new();
static HARNESS_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Build and record the helper once for this test-binary run. Each harness
/// still copies it into a unique private bin directory before launching.
fn locate_solstone_core_binary() -> &'static PathBuf {
    SOLSTONE_CORE_BINARY.get_or_init(build_solstone_core_binary)
}

fn build_solstone_core_binary() -> PathBuf {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("core")
        .join("Cargo.toml");
    let output = Command::new("cargo")
        .args(["build", "--manifest-path"])
        .arg(workspace)
        .args([
            "-p",
            "solstone-core",
            "--bin",
            "solstone-core",
            "--message-format=json",
        ])
        .output()
        .expect("build solstone-core");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] == "compiler-artifact"
            && value["target"]["name"] == "solstone-core"
            && value["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            && let Some(path) = value["executable"].as_str()
        {
            return PathBuf::from(path);
        }
    }
    panic!("cargo did not report solstone-core")
}

struct Harness {
    root: PathBuf,
    binary: PathBuf,
    journal: PathBuf,
    poison: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = HARNESS_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "solstone-importer-cutover-{}-{sequence}-{stamp}",
            std::process::id(),
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let binary = bin.join("solstone-core-journal");
        fs::copy(env!("CARGO_BIN_EXE_solstone-core-journal"), &binary).expect("journal binary");
        executable(&binary);
        let core = bin.join("solstone-core");
        fs::copy(locate_solstone_core_binary(), &core).expect("core binary");
        executable(&core);
        for name in ["python", "python3"] {
            let path = bin.join(name);
            fs::write(&path, POISON_INTERPRETER).expect("poison");
            executable(&path);
        }
        let journal = root.join("journal");
        fs::create_dir_all(&journal).expect("journal");
        Self {
            poison: root.join("poison"),
            root,
            binary,
            journal,
        }
    }

    fn run_importer(&self, args: &[String]) -> Output {
        let _ = fs::remove_file(&self.poison);
        Command::new(&self.binary)
            .arg("importer")
            .args(args)
            .env("POISON_MARKER", &self.poison)
            .env("HOME", self.root.join("home"))
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("PATH", self.root.join("bin"))
            .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
            .env_remove("SOL_SUPERVISOR_SPAWNED")
            .output()
            .expect("run journal importer")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
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

fn executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
}

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
    fn create(journal: &Path) -> Self {
        let directory = journal.join("inputs");
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
        fs::copy(
            root.join("core/fixtures/describe_fiducials/four_tags_16px.png"),
            &image,
        )
        .expect("copy image fixture");
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
        fs::write(
            &kindle,
            "A Book (An Author)\n- Your Highlight on page 1 | Added on Wednesday, March 11, 2026 12:00:00 PM\n\nA highlight\n==========\n",
        )
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

enum Stream {
    Stdout,
    Stderr,
}

struct Case {
    args: Vec<String>,
    exit: i32,
    stream: Stream,
    contains: &'static str,
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn assert_case(harness: &Harness, mode: &str, case: &Case) {
    let _processing = case
        .args
        .iter()
        .any(|argument| {
            Path::new(argument)
                .extension()
                .is_some_and(|extension| extension == "m4a")
        })
        .then(|| AudioProcessingCompleter::start(&harness.journal));
    let _rescan_listener = case
        .args
        .windows(2)
        .any(|pair| pair[0] == "--source" && pair[1] == "journal_archive")
        .then(|| {
            let health = harness.journal.join("health");
            fs::create_dir_all(&health).expect("health directory");
            let socket = health.join("callosum.sock");
            let _ = fs::remove_file(&socket);
            UnixListener::bind(socket).expect("archive rescan listener")
        });
    let output = harness.run_importer(&case.args);
    assert_eq!(
        output.status.code(),
        Some(case.exit),
        "{mode}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (payload, quiet) = match case.stream {
        Stream::Stdout => (&output.stdout, &output.stderr),
        Stream::Stderr => (&output.stderr, &output.stdout),
    };
    assert!(
        String::from_utf8_lossy(payload).contains(case.contains),
        "{mode}: {}",
        String::from_utf8_lossy(payload)
    );
    assert!(quiet.is_empty(), "{mode} wrote to both streams");
    assert!(!harness.poison.exists(), "{mode}: importer reached Python");
}

fn importer_modes(inputs: &Inputs) -> [(&'static str, Vec<Case>); 9] {
    [
        (
            "generic media",
            vec![
                Case {
                    args: vec![path(&inputs.audio), "20260311_120000".to_owned()],
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "day: \"20260311\"",
                },
                Case {
                    args: vec![path(&inputs.text), "20260311_120000".to_owned()],
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Generic text import complete",
                },
                Case {
                    args: vec![
                        "--dry-run".to_owned(),
                        path(&inputs.audio),
                        "20260311_120000".to_owned(),
                    ],
                    exit: 1,
                    stream: Stream::Stderr,
                    contains: "generic audio preview requires the audio import body's preview path",
                },
                Case {
                    args: vec![path(&inputs.text)],
                    exit: 1,
                    stream: Stream::Stderr,
                    contains: "detected timestamp",
                },
                Case {
                    args: vec![
                        "--timestamp".to_owned(),
                        "20260311_120000".to_owned(),
                        path(&inputs.image),
                    ],
                    exit: 1,
                    stream: Stream::Stderr,
                    contains: "automatic source classification requires solstone-core-import-sources registry claims",
                },
            ],
        ),
        (
            "structured sources",
            vec![
                (
                    "ics",
                    &inputs.ics,
                    0,
                    Stream::Stdout,
                    "2 events, 1 unique attendees",
                ),
                (
                    "obsidian",
                    &inputs.vault,
                    0,
                    Stream::Stdout,
                    "1 daily notes, 2 knowledge notes, 1 unique wikilinks",
                ),
                (
                    "document",
                    &inputs.pdf,
                    1,
                    Stream::Stderr,
                    "missing sibling executable",
                ),
                (
                    "image",
                    &inputs.image,
                    0,
                    Stream::Stdout,
                    "image import complete: entries_written=1",
                ),
                (
                    "journal_archive",
                    &inputs.archive,
                    0,
                    Stream::Stdout,
                    "journal_archive import complete: segments_copied=1",
                ),
                (
                    "chatgpt",
                    &inputs.chatgpt,
                    0,
                    Stream::Stdout,
                    "1 messages from ChatGPT export",
                ),
                (
                    "claude",
                    &inputs.claude,
                    0,
                    Stream::Stdout,
                    "2 messages from Claude chat export",
                ),
                (
                    "gemini",
                    &inputs.gemini,
                    0,
                    Stream::Stdout,
                    "1 messages from Gemini export",
                ),
                (
                    "kindle",
                    &inputs.kindle,
                    0,
                    Stream::Stdout,
                    "1 highlights from 1 books",
                ),
            ]
            .into_iter()
            .map(|(source, input, exit, stream, contains)| Case {
                args: {
                    let mut args = vec![
                        "--source".to_owned(),
                        source.to_owned(),
                        "--timestamp".to_owned(),
                        "20260311_120000".to_owned(),
                        path(input),
                    ];
                    if matches!(
                        source,
                        "ics" | "obsidian" | "chatgpt" | "claude" | "gemini" | "kindle"
                    ) {
                        args.push("--dry-run".to_owned());
                    }
                    args
                },
                exit,
                stream,
                contains,
            })
            .collect(),
        ),
        (
            "apple native return",
            vec![Case {
                args: vec![
                    "--source".to_owned(),
                    "apple_health".to_owned(),
                    path(&inputs.apple_export),
                    "--dry-run".to_owned(),
                    "--json".to_owned(),
                ],
                exit: 0,
                stream: Stream::Stdout,
                contains: "\"source\":\"apple_health\"",
            }],
        ),
        (
            "oura file refusal",
            vec![Case {
                args: vec!["--source".to_owned(), "oura".to_owned(), path(&inputs.oura)],
                exit: 1,
                stream: Stream::Stderr,
                contains: "Oura body data imports through sync; use journal importer --sync oura",
            }],
        ),
        (
            "importer listing",
            vec![
                Case {
                    args: args(&["--list-importers"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "File importers:",
                },
                Case {
                    args: args(&["--list-importers", "--json"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "\"journal_archive\"",
                },
            ],
        ),
        (
            "backends",
            vec![Case {
                args: args(&["--backends"]),
                exit: 0,
                stream: Stream::Stdout,
                contains: "Syncable backends:",
            }],
        ),
        (
            "sync",
            vec![
                Case {
                    args: vec![
                        "--sync".to_owned(),
                        "audio".to_owned(),
                        "--path".to_owned(),
                        path(&inputs.audio_directory),
                    ],
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Audio sync preview complete: source=",
                },
                Case {
                    args: vec![
                        "--sync".to_owned(),
                        "obsidian".to_owned(),
                        "--path".to_owned(),
                        path(&inputs.vault),
                    ],
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Obsidian sync preview complete: source=",
                },
                Case {
                    args: args(&["--sync", "plaud", "--save"]),
                    exit: 1,
                    stream: Stream::Stderr,
                    contains: "Plaud sync save requires native credential, download, and import pipeline adapters",
                },
            ],
        ),
        (
            "connect",
            vec![Case {
                args: args(&["--connect", "unknown"]),
                exit: 1,
                stream: Stream::Stderr,
                contains: "Unknown connect backend: unknown",
            }],
        ),
        (
            "journal-source",
            vec![
                Case {
                    args: args(&["journal-source", "create", "sample-source"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Journal source created:",
                },
                Case {
                    args: args(&["journal-source", "list"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "sample-source",
                },
                Case {
                    args: args(&["journal-source", "status", "sample-source"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Journal source: sample-source",
                },
                Case {
                    args: args(&["journal-source", "revoke", "sample-source"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Revoked journal source 'sample-source'",
                },
            ],
        ),
    ]
}

fn run_importer_mode_partition(modes_to_run: &[&str], include_preview_refusals: bool) {
    let harness = Harness::new();
    let inputs = Inputs::create(&harness.journal);

    for (mode, cases) in importer_modes(&inputs) {
        if !modes_to_run.contains(&mode) {
            continue;
        }
        for case in cases {
            assert_case(&harness, mode, &case);
        }
    }
    if !include_preview_refusals {
        return;
    }
    for (source, input) in [
        ("ics", &inputs.ics),
        ("obsidian", &inputs.vault),
        ("claude", &inputs.claude),
        ("chatgpt", &inputs.chatgpt),
        ("kindle", &inputs.kindle),
        ("gemini", &inputs.gemini),
    ] {
        let case = Case {
            args: vec![
                "--source".to_owned(),
                source.to_owned(),
                "--timestamp".to_owned(),
                "20260311_120000".to_owned(),
                path(input),
            ],
            exit: 1,
            stream: Stream::Stderr,
            contains: "previews only and writes nothing; rerun with --dry-run",
        };
        assert_case(&harness, "preview-only refusal", &case);
    }
}

#[test]
fn importer_modes_run_natively_through_the_journal_dispatcher() {
    // These shards have no shared journal, process, or fixture paths. Keep the
    // stateful journal-source workflow inside one shard, while running at most
    // three independently prepared dispatch matrices at once.
    std::thread::scope(|scope| {
        scope.spawn(|| run_importer_mode_partition(&["generic media"], false));
        scope.spawn(|| {
            run_importer_mode_partition(
                &[
                    "structured sources",
                    "apple native return",
                    "oura file refusal",
                ],
                true,
            )
        });
        scope.spawn(|| {
            run_importer_mode_partition(
                &[
                    "importer listing",
                    "backends",
                    "sync",
                    "connect",
                    "journal-source",
                ],
                false,
            )
        });
    });
}

#[test]
fn writing_sources_use_pristine_journals() {
    std::thread::scope(|scope| {
        for (source, expected_file, completion) in [
            (
                "image",
                "image_transcript.md",
                "image import complete: entries_written=1",
            ),
            (
                "journal_archive",
                "value",
                "journal_archive import complete: segments_copied=1",
            ),
        ] {
            scope.spawn(move || {
                let harness = Harness::new();
                let inputs = Inputs::create(&harness.journal);
                let input = match source {
                    "image" => &inputs.image,
                    "journal_archive" => &inputs.archive,
                    _ => unreachable!("writing source table is exhaustive"),
                };
                let case = Case {
                    args: vec![
                        "--source".to_owned(),
                        source.to_owned(),
                        "--timestamp".to_owned(),
                        "20260311_120000".to_owned(),
                        path(input),
                    ],
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: completion,
                };
                assert_case(&harness, "pristine writing source", &case);
                assert!(contains_named_file(
                    &harness.journal.join("chronicle"),
                    expected_file
                ));
            });
        }
    });
}

fn contains_named_file(root: &Path, name: &str) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        path.file_name().is_some_and(|file_name| file_name == name)
            || path.is_dir() && contains_named_file(&path, name)
    })
}
