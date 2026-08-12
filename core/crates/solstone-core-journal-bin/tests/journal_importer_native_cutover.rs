// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const POISON_INTERPRETER: &str = "#!/bin/sh\nprintf '%s\\n' \"$0\" > \"$POISON_MARKER\"\nexit 97\n";

fn locate_solstone_core_binary() -> PathBuf {
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
        let root = env::temp_dir().join(format!(
            "solstone-importer-cutover-{}-{stamp}",
            std::process::id()
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

    fn run_python_token(&self, token: &str) -> Output {
        let _ = fs::remove_file(&self.poison);
        Command::new(&self.binary)
            .arg(token)
            .env("POISON_MARKER", &self.poison)
            .env("HOME", self.root.join("home"))
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("PATH", self.root.join("bin"))
            .output()
            .expect("run Python journal token")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
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
        fs::write(&ics, "BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n").expect("ICS input");
        let vault = directory.join("vault");
        fs::create_dir(&vault).expect("vault directory");
        fs::write(vault.join("note.md"), "# Imported note\n").expect("vault note");
        let pdf = directory.join("document.pdf");
        fs::copy(root.join("core/fixtures/pdf_corpus/text.pdf"), &pdf).expect("copy PDF fixture");
        let image = directory.join("image.png");
        fs::write(
            &image,
            [
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99, 248,
                207, 192, 240, 31, 0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68,
                174, 66, 96, 130,
            ],
        )
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
            text,
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

#[test]
fn importer_modes_run_natively_through_the_journal_dispatcher() {
    let harness = Harness::new();
    let inputs = Inputs::create(&harness.journal);
    let modes = [
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
                    contains: "Generic text import complete:",
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
                    contains: "automatic timestamp detection requires a native timestamp detection adapter",
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
                ("ics", &inputs.ics, "Ics"),
                ("obsidian", &inputs.vault, "Obsidian"),
                ("document", &inputs.pdf, "Document"),
                ("image", &inputs.image, "Image"),
                ("journal_archive", &inputs.archive, "JournalArchive"),
                ("chatgpt", &inputs.archive, "Chatgpt"),
                ("claude", &inputs.archive, "Claude"),
                ("gemini", &inputs.archive, "Gemini"),
                ("kindle", &inputs.kindle, "Kindle"),
            ]
            .into_iter()
            .map(|(source, input, name)| Case {
                args: vec!["--source".to_owned(), source.to_owned(), path(input)],
                exit: 1,
                stream: Stream::Stderr,
                contains: match name {
                    "Ics" => "native importer cannot invoke the Ics source body",
                    "Obsidian" => "native importer cannot invoke the Obsidian source body",
                    "Document" => "native importer cannot invoke the Document source body",
                    "Image" => "native importer cannot invoke the Image source body",
                    "JournalArchive" => {
                        "native importer cannot invoke the JournalArchive source body"
                    }
                    "Chatgpt" => "native importer cannot invoke the Chatgpt source body",
                    "Claude" => "native importer cannot invoke the Claude source body",
                    "Gemini" => "native importer cannot invoke the Gemini source body",
                    "Kindle" => "native importer cannot invoke the Kindle source body",
                    _ => unreachable!("structured source name"),
                },
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
                    args: args(&["journal-source", "create", "w11-source"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Journal source created:",
                },
                Case {
                    args: args(&["journal-source", "list"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "w11-source",
                },
                Case {
                    args: args(&["journal-source", "status", "w11-source"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Journal source: w11-source",
                },
                Case {
                    args: args(&["journal-source", "revoke", "w11-source"]),
                    exit: 0,
                    stream: Stream::Stdout,
                    contains: "Revoked journal source 'w11-source'",
                },
            ],
        ),
    ];

    for (mode, cases) in modes {
        for case in cases {
            assert_case(&harness, mode, &case);
        }
    }
}

#[test]
fn poison_remains_live_for_an_unmigrated_python_process_token() {
    let harness = Harness::new();
    assert_eq!(harness.run_python_token("describe").status.code(), Some(97));
    assert!(harness.poison.exists());
}
