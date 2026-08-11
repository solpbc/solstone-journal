// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod bundle;
mod paths;
pub(crate) mod serialize;
mod validate;

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use bundle::{build_bundle, classify_breaking_changes, read_artifact, repo_relative};
use paths::ContractPaths;
use serialize::render;
use validate::validate_journal_tree;

pub(crate) fn run_build(check: bool, root: Option<PathBuf>) -> ExitCode {
    let paths = match ContractPaths::resolve(root.as_deref()) {
        Ok(paths) => paths,
        Err(error) => return failure(error),
    };
    let bundle = match build_bundle(&paths) {
        Ok(bundle) => bundle,
        Err(error) => return failure(error),
    };
    let expected = render(&bundle);
    if check {
        let current = fs::read_to_string(&paths.artifact).unwrap_or_default();
        if current != expected {
            eprintln!(
                "{} is stale; run `journal contract build`",
                repo_relative(&paths.artifact, &paths.root)
            );
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }
    if let Err(error) = fs::create_dir_all(paths.artifact.parent().expect("artifact has parent")) {
        return failure(format!("contract: cannot create bundle directory: {error}"));
    }
    if let Err(error) = fs::write(&paths.artifact, expected) {
        return failure(format!(
            "contract: cannot write {}: {error}",
            paths.artifact.display()
        ));
    }
    println!("wrote {}", repo_relative(&paths.artifact, &paths.root));
    ExitCode::SUCCESS
}

pub(crate) fn run_check(journals: Vec<PathBuf>, root: Option<PathBuf>) -> ExitCode {
    let paths = match ContractPaths::resolve(root.as_deref()) {
        Ok(paths) => paths,
        Err(error) => return failure(error),
    };
    let current = match build_bundle(&paths) {
        Ok(bundle) => bundle,
        Err(error) => return failure(error),
    };
    if !paths.artifact.is_file() {
        return failure(format!(
            "contract: bundle artifact missing: {}",
            paths.artifact.display()
        ));
    }
    let committed = match read_artifact(&paths.artifact) {
        Ok(bundle) => bundle,
        Err(error) => return failure(error),
    };
    let mut failed = false;
    for change in classify_breaking_changes(&current, &committed) {
        eprintln!("{change}");
        failed = true;
    }
    if render(&current) != fs::read_to_string(&paths.artifact).unwrap_or_default() {
        eprintln!(
            "{} is stale; run `journal contract build`",
            repo_relative(&paths.artifact, &paths.root)
        );
        failed = true;
    }
    if !paths.fixture.join("chronicle").is_dir() {
        eprintln!(
            "contract: journal tree not found: {} (expected {})",
            paths.fixture.display(),
            paths.fixture.join("chronicle").display()
        );
        failed = true;
    } else {
        failed |= report_tree(&paths.fixture, &current);
    }
    for journal in journals {
        // The Python reference treats a missing chronicle directory as an empty
        // tree. Keep its success status, while making the skipped root visible.
        if !journal.join("chronicle").is_dir() {
            eprintln!(
                "contract: no contract-covered files found under {}",
                journal.display()
            );
            continue;
        }
        failed |= report_tree(&journal, &current);
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn report_tree(root: &std::path::Path, bundle: &serde_json::Value) -> bool {
    match validate_journal_tree(root, bundle) {
        Ok(report) => {
            let failed = !report.issues.is_empty();
            for issue in report.issues {
                eprintln!("{}: {}", issue.path, issue.message);
            }
            if report.matched > 0 {
                println!(
                    "validated {} contract-covered files under {}",
                    report.matched,
                    root.display()
                );
            }
            failed
        }
        Err(error) => {
            eprintln!("{error}");
            true
        }
    }
}

fn failure(error: String) -> ExitCode {
    eprintln!("{error}");
    ExitCode::from(1)
}
