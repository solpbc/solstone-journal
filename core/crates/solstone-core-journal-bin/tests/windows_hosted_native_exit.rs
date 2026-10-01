// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A hosted `journal <verb>` returns its native binary's own exit code.
//!
//! On Windows, `journal` cannot replace itself with the native binary named in
//! `NATIVE_PROCESS_SPECS`. When it was itself launched hosted (Sense launching
//! `journal describe` for a segment), it creates that binary through a new
//! hosted launch and waits for the binary to connect and acknowledge. A binary
//! that never completes the handshake is stopped when the deadline passes and
//! `journal` exits 70, so the owner's describe or depict run never happens.
//!
//! Each case launches the real `journal` dispatcher hosted, beside the real
//! native siblings, with an argument the sibling itself rejects, and requires
//! that sibling's own usage exit to come back. An unhosted launch of the same
//! command is the control: it never uses the handshake, so it returns that
//! exit code whether or not the handshake works.
#![cfg(windows)]

#[path = "../../../ci/cargo_environment.rs"]
mod cargo_environment;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use solstone_core_system::process::{
    CommandLaunchRequest, Disposition, HostedLaunchProvenance, LaunchError, launch_command,
    launch_command_hosted,
};

/// What `journal` returns when it could not run the native binary at all.
const LAUNCH_FAILURE_EXIT: i32 = 70;
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

struct Case {
    token: &'static str,
    argument: &'static str,
    exit: i32,
}

// `journal describe` hands `--describe` to the binary first; an unknown
// argument after it is a describe usage error. `journal depict` reports every
// failure, usage included, as 1.
const CASES: &[Case] = &[
    Case {
        token: "describe",
        argument: "--not-a-describe-argument",
        exit: 2,
    },
    Case {
        token: "depict",
        argument: "--not-a-depict-argument",
        exit: 1,
    },
];

struct Install {
    root: PathBuf,
    journal: PathBuf,
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn build_binary(package: &str, binary: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core directory")
        .join("Cargo.toml");
    let output = cargo_environment::cargo_command(env!("CARGO"))
        .args(["build", "--locked", "--manifest-path"])
        .arg(&workspace_manifest)
        .args(["-p", package, "--bin", binary, "--message-format=json"])
        .output()
        .expect("cargo build runs");
    assert!(
        output.status.success(),
        "cargo build -p {package} --bin {binary} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["target"]["name"] == binary)
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
        .unwrap_or_else(|| panic!("cargo build did not report a {binary} executable"))
}

/// The dispatcher and its native siblings, copied into one private directory
/// so `journal` resolves exactly these binaries beside itself.
fn install() -> Install {
    let root = std::env::temp_dir().join(format!(
        "solstone-hosted-native-exit-{}",
        std::process::id()
    ));
    let bin = root.join("bin");
    fs::create_dir_all(&bin).expect("create binary directory");
    fs::create_dir_all(root.join("journal")).expect("create journal directory");
    let install = Install {
        journal: bin.join("journal.exe"),
        root,
    };
    fs::copy(
        env!("CARGO_BIN_EXE_solstone-core-journal"),
        &install.journal,
    )
    .expect("copy journal dispatcher");
    for binary in ["solstone-core-describe", "solstone-core-depict"] {
        fs::copy(
            build_binary(binary, binary),
            bin.join(format!("{binary}.exe")),
        )
        .expect("copy native sibling");
    }
    install
}

fn request(install: &Install, case: &Case) -> CommandLaunchRequest {
    CommandLaunchRequest {
        read_file_grants: Vec::new(),
        program: install.journal.clone().into_os_string(),
        arguments: vec![OsString::from(case.token), OsString::from(case.argument)],
        environment: BTreeMap::new(),
        current_dir: Some(install.root.clone()),
        process_group: false,
        stdin_piped: false,
        stdout_piped: true,
        stderr_piped: true,
    }
}

fn terminate() -> solstone_core_system::process::BoxedTerminateFn {
    Box::new(|child: &mut std::process::Child, _| child.kill().map_err(LaunchError::Terminate))
}

fn disposition() -> Disposition {
    Disposition::IndependentBoundedHelper {
        timeout: RUN_TIMEOUT,
    }
}

fn describe(output: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn unhosted(install: &Install, case: &Case) -> Output {
    launch_command(disposition(), request(install, case), terminate())
        .expect("unhosted journal launch")
        .wait_with_output()
        .expect("unhosted journal output")
}

fn hosted(install: &Install, case: &Case) -> Output {
    launch_command_hosted(
        disposition(),
        request(install, case),
        HostedLaunchProvenance {
            journal: install.root.join("journal"),
            generation: 1,
            launch_id: format!("hosted-native-exit-{}-{}", case.token, std::process::id()),
            service: None,
            parent_launch_id: None,
            acknowledgement_timeout: Duration::from_secs(3),
        },
        terminate(),
    )
    .expect("hosted journal launch and acknowledgement")
    .wait_with_output()
    .expect("hosted journal output")
}

#[test]
fn hosted_journal_returns_the_native_exit_code() {
    let install = install();
    for case in CASES {
        let control = unhosted(&install, case);
        assert_eq!(
            control.status.code(),
            Some(case.exit),
            "unhosted `journal {}` control\n{}",
            case.token,
            describe(&control)
        );

        let output = hosted(&install, case);
        assert_ne!(
            output.status.code(),
            Some(LAUNCH_FAILURE_EXIT),
            "hosted `journal {}` could not launch its native binary\n{}",
            case.token,
            describe(&output)
        );
        assert_eq!(
            output.status.code(),
            Some(case.exit),
            "hosted `journal {}`\n{}",
            case.token,
            describe(&output)
        );
        println!(
            "JOURNAL_WIN_CI_HOSTED_NATIVE_EXIT_{}=PASS",
            case.token.to_ascii_uppercase()
        );
    }
}
