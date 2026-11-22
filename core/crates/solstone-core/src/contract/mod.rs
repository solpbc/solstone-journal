// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod bundle;
pub(crate) mod paths;
pub(crate) mod serialize;
mod validate;

use std::fs;
use std::path::{Path, PathBuf};
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
        let current = match read_or_missing(&paths.artifact) {
            Ok(current) => current,
            Err(error) => return failure(error),
        };
        if current != expected {
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    crate::host_locale(),
                    "journal.cli.host.contract.run.build.path.stale.run.build",
                    [("path", &repo_relative(&paths.artifact, &paths.root))]
                )
            );
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }
    if let Err(error) = fs::create_dir_all(paths.artifact.parent().expect("artifact has parent")) {
        return failure(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.contract.run.build.cannot.create.bundle.directory",
            [("error", &error)],
        ));
    }
    if let Err(error) = fs::write(&paths.artifact, expected) {
        return failure(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.contract.run.build.cannot.write.path",
            [("path", &paths.artifact.display()), ("error", &error)],
        ));
    }
    println!(
        "{}",
        solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.contract.run.build.wrote.path",
            [("path", &repo_relative(&paths.artifact, &paths.root))]
        )
    );
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
        return failure(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.contract.run.check.bundle.artifact.missing.path",
            [("path", &paths.artifact.display())],
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
    let artifact_text = match read_or_missing(&paths.artifact) {
        Ok(text) => text,
        Err(error) => return failure(error),
    };
    if render(&current) != artifact_text {
        eprintln!(
            "{}",
            solstone_core_cli_copy::display_for(
                crate::host_locale(),
                "journal.cli.host.contract.run.build.path.stale.run.build",
                [("path", &repo_relative(&paths.artifact, &paths.root))]
            )
        );
        failed = true;
    }
    match paths.fixture_journal() {
        Ok(fixture) if fixture.join("chronicle").is_dir() => {
            failed |= report_tree(fixture, &current);
        }
        Ok(fixture) => {
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    crate::host_locale(),
                    "journal.cli.host.contract.run.check.tree.not.found.path.expected",
                    [
                        ("path", &fixture.display()),
                        ("expected", &fixture.join("chronicle").display())
                    ]
                )
            );
            failed = true;
        }
        Err(error) => {
            eprintln!("{error}");
            failed = true;
        }
    }
    for journal in journals {
        // The Python reference treats a missing chronicle directory as an empty
        // tree. Keep its success status, while making the skipped root visible.
        if !journal.join("chronicle").is_dir() {
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    crate::host_locale(),
                    "journal.cli.host.contract.run.check.no.covered.files.found.under",
                    [("path", &journal.display())]
                )
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
                    "{}",
                    solstone_core_cli_copy::render_for(
                        crate::host_locale(),
                        "journal.cli.host.contract.validated",
                        &[
                            (
                                "quantity",
                                solstone_core_cli_copy::Value::Cardinal(
                                    crate::host_count_quantity(report.matched as u64)
                                )
                            ),
                            (
                                "display_count",
                                solstone_core_cli_copy::Value::String(&report.matched.to_string())
                            ),
                            (
                                "root",
                                solstone_core_cli_copy::Value::String(&root.display().to_string())
                            )
                        ]
                    )
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

/// A missing artifact is the normal stale state; other read errors need their
/// concrete cause rather than being misreported as staleness.
fn read_or_missing(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.contract.walk.schema.files.cannot.read.path",
            [("path", &path.display()), ("error", &error)],
        )),
    }
}
