// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use solstone_core_installation_identity::{
    ArtifactBindingEvidence, LegacyManifestEvidence, SetupAdmissionRequest, admit_setup,
    journal_token_from_path, namespace_name, owner_base, root_token_from_path,
};
use solstone_core_local::install::metal_candidate::status_targets_native;
use solstone_core_local::install::status::read_status;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: helper <admit|inspect-status|namespace-path> [args...]");
        return ExitCode::from(1);
    }

    match args[1].as_str() {
        "admit" => handle_admit(&args[2..]),
        "inspect-status" => handle_inspect_status(&args[2..]),
        "namespace-path" => handle_namespace_path(&args[2..]),
        other => {
            eprintln!("Unknown subcommand: {other}");
            ExitCode::from(1)
        }
    }
}

fn parse_named_arg(args: &[String], flag: &str) -> Option<PathBuf> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().map(PathBuf::from);
        }
    }
    None
}

fn handle_admit(args: &[String]) -> ExitCode {
    let root = match parse_named_arg(args, "--root") {
        Some(path) => path,
        None => {
            eprintln!("Missing --root");
            return ExitCode::from(1);
        }
    };
    let journal = match parse_named_arg(args, "--journal") {
        Some(path) => path,
        None => {
            eprintln!("Missing --journal");
            return ExitCode::from(1);
        }
    };

    if !root.is_absolute() || !root.is_dir() {
        eprintln!("Root must be an absolute existing directory: {:?}", root);
        return ExitCode::from(1);
    }
    if !journal.is_absolute() || !journal.is_dir() {
        eprintln!("Journal must be an absolute existing directory: {:?}", journal);
        return ExitCode::from(1);
    }

    let root_canonical = match fs::canonicalize(&root) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Failed to canonicalize root: {err}");
            return ExitCode::from(1);
        }
    };
    let journal_canonical = match fs::canonicalize(&journal) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Failed to canonicalize journal: {err}");
            return ExitCode::from(1);
        }
    };

    let owner = match owner_base() {
        Ok(base) => base,
        Err(err) => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": format!("Failed to resolve owner base: {err}")
                })
            );
            return ExitCode::from(1);
        }
    };

    let root_token = match root_token_from_path(&root_canonical) {
        Ok(token) => token,
        Err(err) => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": format!("Failed to create root token: {err}")
                })
            );
            return ExitCode::from(1);
        }
    };

    let journal_token = match journal_token_from_path(&journal_canonical) {
        Ok(token) => token,
        Err(err) => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": format!("Failed to create journal token: {err}")
                })
            );
            return ExitCode::from(1);
        }
    };

    let ns_name = namespace_name(owner.platform(), &root_token);
    let ns_path = owner.path().join("namespaces").join(ns_name.as_hex());

    if ns_path.exists() {
        println!(
            "{}",
            json!({
                "ok": false,
                "error": "namespace_exists",
                "namespace_hex": ns_name.as_hex(),
                "namespace_path": ns_path.display().to_string()
            })
        );
        return ExitCode::from(2);
    }

    let request = SetupAdmissionRequest {
        owner,
        root_token,
        journal_token,
        journal_is_explicit: true,
        legacy_manifest: LegacyManifestEvidence::Absent,
        artifacts: ArtifactBindingEvidence::Fresh,
    };

    match admit_setup(request) {
        Ok(admission) => {
            drop(admission);
            println!(
                "{}",
                json!({
                    "ok": true,
                    "namespace_hex": ns_name.as_hex(),
                    "namespace_path": ns_path.display().to_string(),
                    "root": root_canonical.display().to_string(),
                    "journal": journal_canonical.display().to_string()
                })
            );
            ExitCode::from(0)
        }
        Err(err) => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": format!("Setup admission failed: {err}")
                })
            );
            ExitCode::from(1)
        }
    }
}

fn handle_inspect_status(args: &[String]) -> ExitCode {
    let journal = match parse_named_arg(args, "--journal") {
        Some(path) => path,
        None => {
            eprintln!("Missing --journal");
            return ExitCode::from(1);
        }
    };

    let status_file = journal.join("health/providers/local.json");
    if !status_file.exists() {
        println!(
            "{}",
            json!({
                "ok": false,
                "error": "status file missing"
            })
        );
        return ExitCode::from(1);
    }

    match read_status(&journal, "local") {
        Ok(status) => {
            let native = status_targets_native(&status);
            println!(
                "{}",
                json!({
                    "ok": true,
                    "native": native,
                    "install_state": status.install_state,
                    "target_fingerprint_json": status.target_fingerprint_json,
                    "target_fingerprint_sha256": status.target_fingerprint_sha256,
                    "attempt_id": status.attempt_id
                })
            );
            ExitCode::from(0)
        }
        Err(err) => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": format!("{err}")
                })
            );
            ExitCode::from(1)
        }
    }
}

fn handle_namespace_path(args: &[String]) -> ExitCode {
    let root = match parse_named_arg(args, "--root") {
        Some(path) => path,
        None => {
            eprintln!("Missing --root");
            return ExitCode::from(1);
        }
    };

    if !root.is_absolute() || !root.is_dir() {
        eprintln!("Root must be an absolute existing directory: {:?}", root);
        return ExitCode::from(1);
    }

    let root_canonical = match fs::canonicalize(&root) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Failed to canonicalize root: {err}");
            return ExitCode::from(1);
        }
    };

    let owner = match owner_base() {
        Ok(base) => base,
        Err(err) => {
            eprintln!("Failed to resolve owner base: {err}");
            return ExitCode::from(1);
        }
    };

    let root_token = match root_token_from_path(&root_canonical) {
        Ok(token) => token,
        Err(err) => {
            eprintln!("Failed to create root token: {err}");
            return ExitCode::from(1);
        }
    };

    let ns_name = namespace_name(owner.platform(), &root_token);
    let ns_path = owner.path().join("namespaces").join(ns_name.as_hex());

    println!(
        "{}",
        json!({
            "namespace_hex": ns_name.as_hex(),
            "namespace_path": ns_path.display().to_string()
        })
    );
    ExitCode::from(0)
}
