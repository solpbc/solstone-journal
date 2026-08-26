// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::process::Command;

use solstone_core_cli::{
    CHECK_HELP, CHECK_USAGE, HEALTH_HELP, HEALTH_LOGS_USAGE, INSTALL_MODELS_HELP,
    INSTALL_MODELS_USAGE, INSTALL_PROVIDER_HELP, INSTALL_PROVIDER_USAGE, SPL_HELP, START_HELP,
    START_USAGE, SUPERVISOR_HELP, SUPERVISOR_USAGE,
};
use tempfile::tempdir;

fn run_core(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(args)
        .output()
        .expect("run solstone-core")
}

fn expected_usage_error(usage: &str, command: &str) -> String {
    format!("{usage}{command}: error: invalid arguments\n")
}

fn assert_help(args: &[&str], expected: &str) {
    let output = run_core(args);
    assert_eq!(output.status.code(), Some(0), "{args:?}");
    assert_eq!(output.stderr, b"", "{args:?}");
    assert_eq!(output.stdout, expected.as_bytes(), "{args:?}");
}

#[test]
fn malformed_supervisor_invocation_exits_2_with_its_own_usage() {
    let output = run_core(&["supervisor", "--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_usage_error(SUPERVISOR_USAGE, "journal supervisor")
    );
}
#[test]
fn supervisor_help_is_byte_identical_for_both_spellings() {
    for args in [
        ["supervisor", "--help"].as_slice(),
        ["supervisor", "-h"].as_slice(),
    ] {
        assert_help(args, SUPERVISOR_HELP);
    }
}

#[test]
fn malformed_start_invocation_exits_2_with_its_own_usage() {
    let output = run_core(&["start", "--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_usage_error(START_USAGE, "journal start")
    );
}

#[test]
fn start_help_is_byte_identical_for_both_spellings() {
    for args in [["start", "--help"].as_slice(), ["start", "-h"].as_slice()] {
        assert_help(args, START_HELP);
    }
}

#[test]
fn health_logs_rejects_unknown_flags_with_its_own_usage() {
    let output = run_core(&["health", "logs", "--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_usage_error(HEALTH_LOGS_USAGE, "journal health logs")
    );
}

#[test]
fn health_help_is_byte_identical_for_both_spellings() {
    for args in [["health", "--help"].as_slice(), ["health", "-h"].as_slice()] {
        assert_help(args, HEALTH_HELP);
    }
}

#[test]
fn supervisor_lifecycle_redirects_after_parse_failure() {
    for verb in [
        "start",
        "stop",
        "restart",
        "status",
        "install",
        "uninstall",
        "logs",
    ] {
        let output = run_core(&["supervisor", verb]);
        assert_eq!(output.status.code(), Some(2), "{verb}");
        assert_eq!(output.stdout, b"", "{verb}");
        assert_eq!(
            String::from_utf8(output.stderr).expect("UTF-8 stderr"),
            format!(
                "journal supervisor is the server-launch command (takes a port). \
                 For lifecycle, use: journal service <verb>. Did you mean: journal service {verb} ?\n"
            ),
            "{verb}"
        );
    }

    let output = run_core(&["supervisor", "--wat"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_usage_error(SUPERVISOR_USAGE, "journal supervisor")
    );
}

#[test]
fn malformed_check_invocation_exits_2_with_its_own_usage() {
    let output = run_core(&["check", "--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_usage_error(CHECK_USAGE, "journal check")
    );
}

#[test]
fn spl_help_is_the_spl_help_text_not_an_invalid_choice() {
    for args in [
        ["spl", "--help"].as_slice(),
        ["spl", "-h"].as_slice(),
        ["spl", "service", "--help"].as_slice(),
    ] {
        assert_help(args, SPL_HELP);
    }
}

#[test]
fn check_help_is_byte_identical_for_both_spellings() {
    for args in [
        ["check", "--help"].as_slice(),
        ["check", "-h"].as_slice(),
        ["check", "--json", "--help"].as_slice(),
    ] {
        assert_help(args, CHECK_HELP);
    }
}

#[test]
fn malformed_install_models_invocation_exits_2_with_its_own_usage() {
    let output = run_core(&["install-models", "--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_usage_error(INSTALL_MODELS_USAGE, "journal install-models")
    );
}

#[test]
fn install_models_help_is_byte_identical_for_both_spellings() {
    for args in [
        ["install-models", "--help"].as_slice(),
        ["install-models", "-h"].as_slice(),
    ] {
        assert_help(args, INSTALL_MODELS_HELP);
    }
}

#[test]
fn malformed_install_provider_invocations_exit_2_with_its_own_usage() {
    for args in [
        ["install-provider"].as_slice(),
        ["install-provider", "--wat"].as_slice(),
        ["install-provider", "parakeet", "extra"].as_slice(),
    ] {
        let output = run_core(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
        assert!(
            stderr.starts_with(INSTALL_PROVIDER_USAGE),
            "{args:?} did not print install-provider usage; got:\n{stderr}"
        );
        assert!(
            !stderr.contains("solstone-core"),
            "{args:?} printed top-level usage; got:\n{stderr}"
        );
    }
}

#[test]
fn install_provider_help_is_byte_identical_for_both_spellings() {
    for args in [
        ["install-provider", "--help"].as_slice(),
        ["install-provider", "-h"].as_slice(),
    ] {
        assert_help(args, INSTALL_PROVIDER_HELP);
    }
}

#[test]
fn install_provider_unsupported_name_reaches_the_body() {
    // The verb gates on the journal service before it validates the name, so
    // this case establishes that condition rather than inheriting whatever the
    // build host happens to be running. Without it the assertion grades a
    // supervisor refusal, says nothing about the name arm, and passes or fails
    // according to whether someone left a journal up.
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["install-provider", "bogus"])
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run solstone-core");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        "unsupported provider 'bogus'; supported: local, parakeet\n"
    );
}

#[test]
fn install_provider_gates_on_the_supervisor_before_the_name() {
    // Ordering is the contract: argv errors precede the gate -- which is why
    // the malformed cases above pass with the stack down -- and the gate
    // precedes the name check. A supervisor-spawned child gets 75 and stays
    // silent, because that caller is the one least able to notice a stray line.
    let journal = tempfile::tempdir().expect("temp journal");
    let run = |spawned: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
        command
            .args(["install-provider", "bogus"])
            .env("SOLSTONE_JOURNAL", journal.path())
            .env_remove("SOL_SKIP_SUPERVISOR_CHECK");
        if spawned {
            command.env("SOL_SUPERVISOR_SPAWNED", "1");
        } else {
            command.env_remove("SOL_SUPERVISOR_SPAWNED");
        }
        command.output().expect("run solstone-core")
    };

    let interactive = run(false);
    assert_eq!(interactive.status.code(), Some(1));
    assert!(interactive.stdout.is_empty(), "{:?}", interactive.stdout);
    assert_eq!(
        String::from_utf8(interactive.stderr).expect("UTF-8 stderr"),
        "journal isn't running. start it with 'journal up' and retry.\n"
    );

    let spawned = run(true);
    assert_eq!(spawned.status.code(), Some(75));
    assert!(spawned.stdout.is_empty(), "{:?}", spawned.stdout);
    assert_eq!(spawned.stderr, b"");
}

// --- transfer -------------------------------------------------------------
//
// `journal transfer` had the same defect and worse: EVERY invocation, including
// --help, exited 64 with solstone-core's top-level usage. The verb shipped with
// no help at all.

const TRANSFER_MALFORMED: &[&[&str]] = &[
    &["transfer"],
    &["transfer", "--nonsense"],
    &["transfer", "bogus"],
];

#[test]
fn malformed_transfer_invocations_exit_2_not_64() {
    for args in TRANSFER_MALFORMED {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(*args)
            .output()
            .expect("run solstone-core");
        let code = output.status.code().expect("exit code");
        assert_eq!(
            code,
            2,
            "`{}` exited {code}; the reference exits 2 (argparse usage error)",
            args.join(" ")
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("usage: journal transfer"),
            "`{}` did not print journal transfer's usage; got:\n{stderr}",
            args.join(" ")
        );
        assert!(
            !stderr.contains("solstone-core --version"),
            "`{}` printed solstone-core's top-level usage instead of the verb's; \
             got:\n{stderr}",
            args.join(" ")
        );
        assert!(stderr.contains("{send}"), "{stderr}");
        assert!(!stderr.contains("{export,import,send}"), "{stderr}");
    }
}

#[test]
fn transfer_help_advertises_only_the_live_send_subcommand() {
    for (args, expected_usage) in [
        (
            ["transfer", "--help"].as_slice(),
            "usage: journal transfer [-h]",
        ),
        (
            ["transfer", "-h"].as_slice(),
            "usage: journal transfer [-h]",
        ),
        (
            ["transfer", "send", "--help"].as_slice(),
            "usage: journal transfer send",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .output()
            .expect("run solstone-core");
        assert_eq!(
            output.status.code(),
            Some(0),
            "`{}` did not exit 0; the reference serves help here",
            args.join(" ")
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.starts_with(expected_usage),
            "`{}` did not print its own help (expected it to start {expected_usage:?}); \
             got:\n{stdout}",
            args.join(" ")
        );
        if args.len() == 2 && args[0] == "transfer" {
            assert!(stdout.contains("{send}"), "{stdout}");
            assert!(!stdout.contains("{export,import,send}"), "{stdout}");
        }
    }
}

#[test]
fn retired_transfer_subcommands_exit_2_with_their_replacements() {
    for (args, replacement) in [
        (["transfer", "export"].as_slice(), "journal archive export"),
        (
            ["transfer", "export", "--help"].as_slice(),
            "journal archive export",
        ),
        (["transfer", "import"].as_slice(), "journal archive merge"),
        (
            ["transfer", "import", "--help"].as_slice(),
            "journal archive merge",
        ),
    ] {
        let output = run_core(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(replacement), "{stderr}");
        assert!(!stderr.contains("usage: journal transfer"), "{stderr}");
    }
}

// --- export ---------------------------------------------------------------
//
// Export is retired at both its ordinary and help spellings. Its tombstone
// must not fall through to solstone-core's generic top-level usage.

#[test]
fn direct_export_tombstone_exits_2_with_send_replacement() {
    for args in [["export"].as_slice(), ["export", "--help"].as_slice()] {
        let output = run_core(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("journal transfer send --to"),
            "`{}` did not print the export replacement; got:\n{stderr}",
            args.join(" ")
        );
        assert!(
            !stderr.contains("solstone-core --version"),
            "`{}` printed only solstone-core's top-level usage; got:\n{stderr}",
            args.join(" ")
        );
    }
}

// --- navigate -------------------------------------------------------------

#[test]
fn malformed_navigate_invocations_exit_2_with_their_own_usage() {
    for args in [
        ["navigate", "--nonsense"].as_slice(),
        ["navigate", "/a", "/b"].as_slice(),
        ["navigate"].as_slice(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .output()
            .expect("run solstone-core");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("usage: journal navigate"),
            "{args:?} did not print navigate usage: {stderr}"
        );
        assert!(
            stderr.contains("journal navigate: error: invalid arguments"),
            "{args:?} did not print navigate error: {stderr}"
        );
        assert!(
            !stderr.contains("solstone-core --version"),
            "{args:?} printed top-level usage: {stderr}"
        );
        assert!(
            !stderr.contains("solstone-core"),
            "{args:?} named the internal binary: {stderr}"
        );
    }
}

#[test]
fn navigate_help_is_served_not_treated_as_a_usage_error() {
    for args in [
        ["navigate", "--help"].as_slice(),
        ["navigate", "-h"].as_slice(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .output()
            .expect("run solstone-core");
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(output.stderr, b"", "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.starts_with("usage: journal navigate"),
            "{args:?} did not print navigate help: {stdout}"
        );
        assert!(!stdout.contains("facet"), "{stdout}");
        assert!(stdout.contains("positional arguments:"), "{stdout}");
        assert!(
            stdout
                .lines()
                .any(|line| line.trim_start().starts_with("PATH")),
            "{stdout}"
        );
        assert!(!stdout.contains("solstone-core --version"), "{stdout}");
        assert!(!stdout.contains("solstone-core"), "{stdout}");
    }
}

// --- identity --------------------------------------------------------------

#[test]
fn malformed_identity_invocations_exit_2_with_their_own_usage() {
    for (args, usage, error) in [
        (
            ["identity", "bogus"].as_slice(),
            "usage: journal identity",
            "error: invalid choice: 'bogus'",
        ),
        (
            ["identity", "partner", "--nonsense"].as_slice(),
            "usage: journal identity partner",
            "error: invalid arguments",
        ),
        (
            ["identity", "health", "--nonsense"].as_slice(),
            "usage: journal identity health",
            "error: invalid arguments",
        ),
        (
            ["identity", "briefing", "--day", "bad"].as_slice(),
            "usage: journal identity briefing",
            "error: invalid arguments",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .output()
            .expect("run solstone-core identity");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with(usage), "{args:?}: {stderr}");
        assert!(stderr.contains(error), "{args:?}: {stderr}");
        assert!(!stderr.contains("solstone-core"), "{args:?}: {stderr}");
    }
}

// --- transcribe -----------------------------------------------------------

const TRANSCRIBE_USAGE: &str = concat!(
    "usage: journal transcribe [-h] [--all] [--redo]\n",
    "                          [--backend {parakeet,parakeet-cpp,confidential}]\n",
    "                          [-v] [-d]\n",
    "                          [audio_path]\n",
);

const TRANSCRIBE_HELP: &str = concat!(
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

fn run_transcribe(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["transcribe"])
        .args(args)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run solstone-core transcribe")
}

fn expected_transcribe_error(message: &str) -> String {
    format!("{TRANSCRIBE_USAGE}journal transcribe: error: {message}\n")
}

#[test]
fn malformed_transcribe_invocation_exits_2_with_its_own_usage() {
    let output = run_transcribe(&["--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_transcribe_error("unrecognized arguments: --nonsense")
    );
}

#[test]
fn transcribe_help_is_byte_identical_for_both_spellings() {
    for args in [["--help"].as_slice(), ["-h"].as_slice()] {
        let output = run_transcribe(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(output.stderr, b"", "{args:?}");
        assert_eq!(
            String::from_utf8(output.stdout).expect("UTF-8 stdout"),
            TRANSCRIBE_HELP,
            "{args:?}"
        );
    }
}

#[test]
fn invalid_transcribe_backend_exits_2_with_argparse_choice_error() {
    let output = run_transcribe(&["--backend", "not-a-backend", "--all"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 stderr"),
        expected_transcribe_error(
            "argument --backend: invalid choice: 'not-a-backend' (choose from parakeet, parakeet-cpp, confidential)"
        )
    );
}

#[test]
fn transcribe_backend_value_does_not_consume_logging_flag() {
    let output = run_transcribe(&["--backend", "--debug", "parakeet", "--all"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
}

#[test]
fn transcribe_selection_errors_match_the_reference() {
    for (args, message) in [
        (
            ["--all", "some/audio.wav"].as_slice(),
            "--all and audio_path are mutually exclusive",
        ),
        ([].as_slice(), "provide audio_path or --all"),
    ] {
        let output = run_transcribe(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).expect("UTF-8 stderr"),
            expected_transcribe_error(message),
            "{args:?}"
        );
    }
}

// --- facet-candidates -----------------------------------------------------

#[test]
fn malformed_facet_candidates_invocations_exit_2_before_supervisor_preflight() {
    for args in [
        ["facet-candidates", "--nonsense"].as_slice(),
        ["facet-candidates", "extra-positional"].as_slice(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .env_remove("SOLSTONE_JOURNAL")
            .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
            .env_remove("SOL_SUPERVISOR_SPAWNED")
            .output()
            .expect("run solstone-core facet-candidates");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
        assert!(
            stderr.contains("usage: journal facet-candidates"),
            "{stderr}"
        );
        assert!(!stderr.contains("solstone-core --version"), "{stderr}");
    }
}

#[test]
fn facet_candidates_help_is_structural_and_served_before_journal_resolution() {
    for args in [["facet-candidates", "--help"], ["facet-candidates", "-h"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .env_remove("SOLSTONE_JOURNAL")
            .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
            .env_remove("SOL_SUPERVISOR_SPAWNED")
            .output()
            .expect("run solstone-core facet-candidates");
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(output.stderr, b"", "{args:?}");
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
        assert!(
            stdout.starts_with("usage: journal facet-candidates [-h] [-v] [-d]\n"),
            "{stdout}"
        );
        assert!(stdout.contains("-h, --help"), "{stdout}");
        assert!(stdout.contains("-v, --verbose"), "{stdout}");
        assert!(stdout.contains("-d, --debug"), "{stdout}");
        assert!(
            stdout.contains("Record recurring facet review candidates"),
            "{stdout}"
        );
    }
}

fn run_facet_candidates(args: &[&str], journal: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .arg("facet-candidates")
        .args(args)
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env_remove("SOL_SUPERVISOR_SPAWNED")
        .output()
        .expect("run solstone-core facet-candidates")
}

#[test]
fn facet_candidates_logging_flags_do_not_change_output() {
    let journal = tempdir().expect("create journal");
    let baseline = run_facet_candidates(&[], journal.path());
    assert_eq!(baseline.status.code(), Some(0));
    for args in [
        ["-v"].as_slice(),
        ["-d"].as_slice(),
        ["-v", "-d"].as_slice(),
    ] {
        let output = run_facet_candidates(args, journal.path());
        assert_eq!(output.status.code(), baseline.status.code(), "{args:?}");
        assert_eq!(output.stdout, baseline.stdout, "{args:?}");
        assert_eq!(output.stderr, baseline.stderr, "{args:?}");
    }
}
