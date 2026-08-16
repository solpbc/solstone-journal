// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_repository_contracts::ci::{
    load_boundary, load_registry, scan_routine_boundaries, validate_boundary, validate_registry,
};
use std::env;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("solstone-ci: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "plan".to_owned());
    if args.next().is_some() {
        return Err("this command does not accept positional arguments".to_owned());
    }
    let repo = repo_root()?;
    let registry_path = repo.join("core/ci/suites.toml");
    let boundary_path = repo.join("core/ci/routine-boundaries.toml");

    match command.as_str() {
        "validate" => {
            let registry = load_registry(&registry_path)?;
            let boundary = load_boundary(&boundary_path)?;
            let mut errors = Vec::new();
            if let Err(found) = validate_registry(&repo, &registry) {
                errors.extend(found);
            }
            if let Err(found) = validate_boundary(&repo, &boundary) {
                errors.extend(found);
            }
            if errors.is_empty() {
                println!(
                    "CI topology valid: {} Cargo integration targets, {} named legs, {} routine-boundary findings",
                    registry.suites.len(),
                    registry.legs.len(),
                    boundary.findings.len()
                );
                Ok(())
            } else {
                for error in &errors {
                    eprintln!("- {error}");
                }
                Err(format!("CI topology has {} error(s)", errors.len()))
            }
        }
        "plan" => {
            let registry = load_registry(&registry_path)?;
            validate_registry(&repo, &registry).map_err(|errors| errors.join("\n"))?;
            println!("Default full CI plan (read-only)");
            for leg in registry.legs.iter().filter(|leg| leg.default_full) {
                println!(
                    "leg\t{}\tset={}\tareas={}\tplatforms={}\ttimeout={}",
                    leg.id,
                    leg.set,
                    leg.areas.join(","),
                    leg.platforms.join(","),
                    leg.timeout
                );
            }
            for suite in registry.suites.iter().filter(|suite| suite.default_full) {
                println!(
                    "suite\t{}\tset={}\tareas={}\tplatforms={}\ttimeout={}",
                    suite.id,
                    suite.set,
                    suite.areas.join(","),
                    suite.platforms.join(","),
                    suite.timeout
                );
            }
            Ok(())
        }
        "boundary-snapshot" => {
            println!("version = 1");
            for id in scan_routine_boundaries(&repo)? {
                println!("\n[[findings]]");
                println!(
                    "id = {}",
                    serde_json::to_string(&id).map_err(|error| error.to_string())?
                );
            }
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("usage: solstone-ci [validate|plan|boundary-snapshot]");
            Ok(())
        }
        _ => Err(format!("unknown command {command:?}")),
    }
}

fn repo_root() -> Result<PathBuf, String> {
    let mut current = env::current_dir().map_err(|error| format!("current directory: {error}"))?;
    loop {
        if is_repo_root(&current) {
            return Ok(current);
        }
        if !current.pop() {
            return Err("run from a solstone-journal checkout".to_owned());
        }
    }
}

fn is_repo_root(path: &Path) -> bool {
    path.join("Makefile").is_file() && path.join("core/Cargo.toml").is_file()
}
