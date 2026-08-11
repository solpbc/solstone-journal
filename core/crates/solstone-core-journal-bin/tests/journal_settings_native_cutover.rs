// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s\n' "$0" > "$POISON_MARKER"
exit 97
"#;

const ROOT_HELP: &str = concat!(
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
const CONVEY_HELP: &str = concat!(
    "usage: journal settings convey [-h] {status} ...\n",
    "\n",
    "positional arguments:\n",
    "  {status}\n",
    "    status    Show convey bind and dashboard URL status\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
);
const STATUS_HELP: &str = concat!(
    "usage: journal settings convey status [-h] [--json]\n",
    "\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --json      Print machine-readable status.\n",
);
const STATUS_5051: &str =
    "convey\n  bind:              127.0.0.1:5051\n  dashboard url:     http://localhost:5051\n";
const JSON_5051: &str = "{\n  \"dashboard_url\": \"http://localhost:5051\"\n}\n";
const JSON_5015: &str = "{\n  \"dashboard_url\": \"http://localhost:5015\"\n}\n";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-journal-settings-cutover-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Harness {
    temp: TempDir,
    binary: PathBuf,
    home: PathBuf,
    journal: PathBuf,
    poison_marker: PathBuf,
}

enum JournalEnv<'a> {
    Set(&'a Path),
    Unset,
    Empty,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new();
        let bin = temp.path.join("bin");
        fs::create_dir(&bin).expect("create binary directory");

        let binary = bin.join("solstone-core-journal");
        fs::copy(env!("CARGO_BIN_EXE_solstone-core-journal"), &binary)
            .expect("copy native journal binary");
        make_executable(&binary);

        let core = bin.join("solstone-core");
        fs::copy(locate_solstone_core_binary(), &core).expect("copy native core binary");
        make_executable(&core);

        for interpreter in ["python", "python3", "pytest", "uv", "ruff"] {
            let path = bin.join(interpreter);
            fs::write(&path, POISON_INTERPRETER).expect("write poison interpreter");
            make_executable(&path);
        }

        let journal = temp.path.join("journal");
        fs::create_dir(&journal).expect("create journal");
        Self {
            home: temp.path.join("home"),
            poison_marker: temp.path.join("python-invoked.txt"),
            temp,
            binary,
            journal,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_environment(args, JournalEnv::Set(&self.journal))
    }

    fn run_with_environment(&self, args: &[&str], journal_env: JournalEnv<'_>) -> Output {
        let _ = fs::remove_file(&self.poison_marker);
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .env("POISON_MARKER", &self.poison_marker)
            .env("HOME", &self.home)
            .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
            .env_remove("SOL_SUPERVISOR_SPAWNED");
        match journal_env {
            JournalEnv::Set(path) => {
                command.env("SOLSTONE_JOURNAL", path);
            }
            JournalEnv::Unset => {
                command.env_remove("SOLSTONE_JOURNAL");
            }
            JournalEnv::Empty => {
                command.env("SOLSTONE_JOURNAL", "");
            }
        }
        command.output().expect("run native journal process")
    }

    fn write_port(&self, journal: &Path, value: &str) {
        let health = journal.join("health");
        fs::create_dir_all(&health).expect("create health directory");
        fs::write(health.join("convey.port"), value).expect("write port");
    }

    fn remove_port(&self, journal: &Path) {
        let path = journal.join("health/convey.port");
        if path.is_dir() {
            fs::remove_dir(&path).expect("remove port directory");
        } else {
            let _ = fs::remove_file(path);
        }
    }

    fn assert_python_was_not_invoked(&self) {
        assert!(
            !self.poison_marker.exists(),
            "native dispatch invoked a poisoned Python interpreter"
        );
    }
}

fn locate_solstone_core_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates directory")
        .parent()
        .expect("core directory")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(workspace_manifest)
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
        "cargo build -p solstone-core failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let target = &message["target"];
        if message["reason"] == "compiler-artifact"
            && target["name"] == "solstone-core"
            && target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            && let Some(path) = message["executable"].as_str()
        {
            return PathBuf::from(path);
        }
    }
    panic!("cargo did not report solstone-core")
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
}

struct Case {
    args: &'static [&'static str],
    exit: i32,
    stdout: &'static str,
    stderr: &'static str,
}

#[test]
fn settings_grammar_matches_the_pinned_reference_without_python() {
    let harness = Harness::new();
    harness.write_port(&harness.journal, "5051");
    let root_unrecognized = "usage: journal settings [-h] [-v] [-d] {convey} ...\n\
journal settings: error: unrecognized arguments: --nonsense\n";
    let root_bogus = "usage: journal settings [-h] [-v] [-d] {convey} ...\n\
journal settings: error: argument section: invalid choice: 'bogus' (choose from convey)\n";
    let convey_bogus = "usage: journal settings convey [-h] {status} ...\n\
journal settings convey: error: argument convey_command: invalid choice: 'bogus' (choose from status)\n";
    let root_flag = "usage: journal settings [-h] [-v] [-d] {convey} ...\n\
journal settings: error: unrecognized arguments: -v\n";
    let mut cases = vec![
        Case {
            args: &[],
            exit: 1,
            stdout: ROOT_HELP,
            stderr: "",
        },
        Case {
            args: &["-h"],
            exit: 0,
            stdout: ROOT_HELP,
            stderr: "",
        },
        Case {
            args: &["--help"],
            exit: 0,
            stdout: ROOT_HELP,
            stderr: "",
        },
        Case {
            args: &["convey", "-h"],
            exit: 0,
            stdout: CONVEY_HELP,
            stderr: "",
        },
        Case {
            args: &["convey", "--help"],
            exit: 0,
            stdout: CONVEY_HELP,
            stderr: "",
        },
        Case {
            args: &["convey", "status", "-h"],
            exit: 0,
            stdout: STATUS_HELP,
            stderr: "",
        },
        Case {
            args: &["convey", "status", "--help"],
            exit: 0,
            stdout: STATUS_HELP,
            stderr: "",
        },
        Case {
            args: &["convey"],
            exit: 1,
            stdout: CONVEY_HELP,
            stderr: "",
        },
        Case {
            args: &["convey", "status"],
            exit: 0,
            stdout: STATUS_5051,
            stderr: "",
        },
        Case {
            args: &["convey", "status", "--json"],
            exit: 0,
            stdout: JSON_5051,
            stderr: "",
        },
        Case {
            args: &["--nonsense"],
            exit: 2,
            stdout: "",
            stderr: root_unrecognized,
        },
        Case {
            args: &["bogus"],
            exit: 2,
            stdout: "",
            stderr: root_bogus,
        },
        Case {
            args: &["convey", "bogus"],
            exit: 2,
            stdout: "",
            stderr: convey_bogus,
        },
        Case {
            args: &["convey", "status", "-v"],
            exit: 2,
            stdout: "",
            stderr: root_flag,
        },
        Case {
            args: &["convey", "-v", "status"],
            exit: 2,
            stdout: "",
            stderr: root_flag,
        },
    ];
    for flag in ["-v", "--verbose", "-d", "--debug"] {
        cases.push(Case {
            args: Box::leak(vec![flag].into_boxed_slice()),
            exit: 1,
            stdout: ROOT_HELP,
            stderr: "",
        });
        cases.push(Case {
            args: Box::leak(vec![flag, "convey"].into_boxed_slice()),
            exit: 1,
            stdout: CONVEY_HELP,
            stderr: "",
        });
        cases.push(Case {
            args: Box::leak(vec![flag, "convey", "status"].into_boxed_slice()),
            exit: 0,
            stdout: STATUS_5051,
            stderr: "",
        });
        cases.push(Case {
            args: Box::leak(vec![flag, "convey", "status", "--json"].into_boxed_slice()),
            exit: 0,
            stdout: JSON_5051,
            stderr: "",
        });
    }

    for case in cases {
        let mut args = vec!["settings"];
        args.extend_from_slice(case.args);
        let output = harness.run(&args);
        assert_eq!(output.status.code(), Some(case.exit), "{:?}", case.args);
        assert_eq!(output.stdout, case.stdout.as_bytes(), "{:?}", case.args);
        assert_eq!(output.stderr, case.stderr.as_bytes(), "{:?}", case.args);
        harness.assert_python_was_not_invoked();
    }
}

#[test]
fn settings_port_table_preserves_python_values_and_rejects_directory_errors() {
    let harness = Harness::new();
    for (value, expected) in [
        (Some("5051"), JSON_5051),
        (None, JSON_5015),
        (Some(""), JSON_5015),
        (Some("abc"), JSON_5015),
        (Some("0"), JSON_5015),
        (
            Some("70000"),
            "{\n  \"dashboard_url\": \"http://localhost:70000\"\n}\n",
        ),
        (
            Some("-1"),
            "{\n  \"dashboard_url\": \"http://localhost:-1\"\n}\n",
        ),
        (Some("5_051"), JSON_5051),
    ] {
        harness.remove_port(&harness.journal);
        if let Some(value) = value {
            harness.write_port(&harness.journal, value);
        }
        let output = harness.run(&["settings", "convey", "status", "--json"]);
        assert_eq!(output.status.code(), Some(0), "{value:?}");
        assert_eq!(output.stdout, expected.as_bytes(), "{value:?}");
        assert_eq!(output.stderr, b"", "{value:?}");
        harness.assert_python_was_not_invoked();
    }

    harness.remove_port(&harness.journal);
    fs::create_dir_all(harness.journal.join("health/convey.port")).expect("create port directory");
    let output = harness.run(&["settings", "convey", "status", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("Error: "),
        "{:?}",
        output.stderr
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("5015"));
    harness.assert_python_was_not_invoked();
}

#[test]
fn settings_resolves_env_and_config_journal_paths_in_child_processes() {
    let harness = Harness::new();
    let env_journal = harness.temp.path.join("env-journal");
    let config_journal = harness.temp.path.join("config-journal");
    harness.write_port(&env_journal, "70000");
    harness.write_port(&config_journal, "5051");
    let config_dir = harness.home.join(".config/solstone");
    fs::create_dir_all(&config_dir).expect("create config directory");
    fs::write(
        config_dir.join("config.toml"),
        format!("journal = \"{}\"\n", config_journal.display()),
    )
    .expect("write journal config");

    let env_output = harness.run_with_environment(
        &["settings", "convey", "status", "--json"],
        JournalEnv::Set(&env_journal),
    );
    assert_eq!(env_output.status.code(), Some(0));
    assert_eq!(
        env_output.stdout,
        b"{\n  \"dashboard_url\": \"http://localhost:70000\"\n}\n"
    );
    harness.assert_python_was_not_invoked();

    for journal_env in [JournalEnv::Unset, JournalEnv::Empty] {
        let output =
            harness.run_with_environment(&["settings", "convey", "status", "--json"], journal_env);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stdout, JSON_5051.as_bytes());
        assert_eq!(output.stderr, b"");
        harness.assert_python_was_not_invoked();
    }
}

#[test]
fn settings_creates_journal_only_after_successful_parsing() {
    let harness = Harness::new();
    for (name, args, exit, should_create) in [
        ("root-help", &["settings", "-h"][..], 0, false),
        ("convey-help", &["settings", "convey", "-h"][..], 0, false),
        (
            "status-help",
            &["settings", "convey", "status", "-h"][..],
            0,
            false,
        ),
        ("argument-error", &["settings", "--nonsense"][..], 2, false),
        ("root-fallback", &["settings"][..], 1, true),
        ("convey-fallback", &["settings", "convey"][..], 1, true),
        (
            "status",
            &["settings", "convey", "status", "--json"][..],
            0,
            true,
        ),
    ] {
        let journal = harness.temp.path.join(format!("missing-{name}"));
        let output = harness.run_with_environment(args, JournalEnv::Set(&journal));
        assert_eq!(output.status.code(), Some(exit), "{name}");
        assert_eq!(journal.is_dir(), should_create, "{name}");
        harness.assert_python_was_not_invoked();
    }
}

#[test]
fn poison_remains_live_for_a_python_token() {
    let harness = Harness::new();
    let output = harness.run(&["describe"]);

    assert_eq!(output.status.code(), Some(97));
    assert!(
        harness.poison_marker.exists(),
        "describe did not invoke the poisoned interpreter"
    );
}
