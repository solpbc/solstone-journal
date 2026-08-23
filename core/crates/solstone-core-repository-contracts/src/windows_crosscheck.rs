// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

#[derive(Debug, Deserialize)]
pub struct CrosscheckConfig {
    version: u32,
    target: String,
    baseline: Baseline,
    exclusions: Vec<Exclusion>,
}

#[derive(Debug, Deserialize)]
struct Baseline {
    workspace_packages: usize,
    library_packages: usize,
}

#[derive(Debug, Deserialize)]
struct Exclusion {
    package: String,
    version: Option<String>,
    class: String,
    reason: String,
    expected_stderr: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    targets: Vec<Target>,
}

#[derive(Debug, Deserialize)]
struct Target {
    kind: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PackageKey {
    name: String,
    version: String,
}

#[derive(Clone, Copy)]
enum Sweep {
    Library,
    Tests,
}

impl Sweep {
    fn label(self) -> &'static str {
        match self {
            Self::Library => "lib",
            Self::Tests => "tests",
        }
    }

    fn includes_dev(self) -> bool {
        matches!(self, Self::Tests)
    }
}

pub fn run(repo: &Path, config_path: &Path) -> Result<(), String> {
    let config = load_config(config_path)?;
    let metadata = cargo_metadata(repo, &config.target)?;
    validate_census(repo, &metadata, &config)?;
    let exclusions = resolve_exclusions(&metadata, &config.exclusions)?;

    println!("Windows cross-check target: {}", config.target);
    println!("Probing {} documented exclusion roots", exclusions.len());
    for (exclusion, package) in &exclusions {
        probe_exclusion(repo, &config.target, exclusion, package)?;
    }

    sweep(repo, &config.target, &metadata, &exclusions, Sweep::Library)?;
    sweep(repo, &config.target, &metadata, &exclusions, Sweep::Tests)?;
    Ok(())
}

fn load_config(path: &Path) -> Result<CrosscheckConfig, String> {
    let text = fs::read_to_string(path).map_err(|error| {
        format!(
            "read Windows cross-check config {}: {error}",
            path.display()
        )
    })?;
    let config: CrosscheckConfig = toml_edit::de::from_str(&text).map_err(|error| {
        format!(
            "parse Windows cross-check config {}: {error}",
            path.display()
        )
    })?;
    if config.version != 1 {
        return Err(format!(
            "unsupported Windows cross-check config version {}",
            config.version
        ));
    }
    if config.target != "x86_64-pc-windows-msvc" {
        return Err(format!(
            "Windows cross-check target must remain x86_64-pc-windows-msvc, found {}",
            config.target
        ));
    }
    if config.exclusions.is_empty() {
        return Err("Windows cross-check exclusion instrument is empty".to_owned());
    }
    Ok(config)
}

fn cargo_metadata(repo: &Path, target: &str) -> Result<Metadata, String> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            "core/Cargo.toml",
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--filter-platform",
            target,
        ])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("start cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Windows cargo metadata failed:\n{}",
            output_text(&output)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse Windows cargo metadata: {error}"))
}

fn validate_census(
    repo: &Path,
    metadata: &Metadata,
    config: &CrosscheckConfig,
) -> Result<(), String> {
    let workspace = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
    let libraries = metadata
        .packages
        .iter()
        .filter(|package| {
            workspace.contains(&package.id)
                && package
                    .targets
                    .iter()
                    .any(|target| target.kind.iter().any(|kind| kind == "lib"))
        })
        .count();
    if workspace.len() != config.baseline.workspace_packages
        || libraries != config.baseline.library_packages
    {
        return Err(format!(
            "Windows cross-check census drifted: expected {} workspace / {} library packages, found {} / {}; review the new package and update the measured baseline",
            config.baseline.workspace_packages,
            config.baseline.library_packages,
            workspace.len(),
            libraries
        ));
    }
    let closure = dependency_packages(
        repo,
        &config.target,
        "solstone-core-journal-config",
        Sweep::Library,
    )?;
    for (dependency, version) in [("whoami", "2.1.2"), ("iana-time-zone", "0.1.65")] {
        if !closure
            .iter()
            .any(|package| package.name == dependency && package.version == version)
        {
            return Err(format!(
                "cargo-tree control failed: journal-config Windows closure does not contain {dependency} {version}"
            ));
        }
    }
    Ok(())
}

fn resolve_exclusions<'a>(
    metadata: &Metadata,
    exclusions: &'a [Exclusion],
) -> Result<Vec<(&'a Exclusion, PackageKey)>, String> {
    let mut resolved = Vec::new();
    let mut ids = BTreeSet::new();
    for exclusion in exclusions {
        if exclusion.class.trim().is_empty()
            || exclusion.reason.trim().is_empty()
            || exclusion.expected_stderr.is_empty()
        {
            return Err(format!(
                "exclusion {} must name a class, reason, and expected stderr",
                exclusion.package
            ));
        }
        let matches = metadata
            .packages
            .iter()
            .filter(|package| {
                package.name == exclusion.package
                    && exclusion
                        .version
                        .as_ref()
                        .is_none_or(|version| &package.version == version)
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(format!(
                "exclusion {}{} resolved to {} packages instead of exactly one",
                exclusion.package,
                exclusion
                    .version
                    .as_ref()
                    .map_or_else(String::new, |version| format!("@{version}")),
                matches.len()
            ));
        }
        let key = PackageKey {
            name: matches[0].name.clone(),
            version: matches[0].version.clone(),
        };
        if !ids.insert(key.clone()) {
            return Err(format!(
                "duplicate exclusion root {}@{}",
                key.name, key.version
            ));
        }
        resolved.push((exclusion, key));
    }
    Ok(resolved)
}

fn probe_exclusion(
    repo: &Path,
    target: &str,
    exclusion: &Exclusion,
    package: &PackageKey,
) -> Result<(), String> {
    let spec = exclusion.version.as_ref().map_or_else(
        || package.name.clone(),
        |version| format!("{}@{version}", package.name),
    );
    let output = cargo_check(repo, target, &spec, Sweep::Library)?;
    let text = output_text(&output);
    if output.status.success() {
        return Err(format!(
            "stale Windows exclusion {} ({}) now passes; remove or narrow it: {}",
            exclusion.package, exclusion.class, exclusion.reason
        ));
    }
    let missing = exclusion
        .expected_stderr
        .iter()
        .filter(|needle| !text.contains(needle.as_str()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Windows exclusion {} failed differently; missing {:?}\n{}",
            exclusion.package, missing, text
        ));
    }
    println!(
        "EXCLUDED root {} [{}]: {}",
        exclusion.package, exclusion.class, exclusion.reason
    );
    Ok(())
}

fn sweep(
    repo: &Path,
    target: &str,
    metadata: &Metadata,
    exclusions: &[(&Exclusion, PackageKey)],
    sweep: Sweep,
) -> Result<(), String> {
    let workspace = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
    let packages = metadata
        .packages
        .iter()
        .filter(|package| workspace.contains(&package.id))
        .filter(|package| {
            sweep.includes_dev()
                || package
                    .targets
                    .iter()
                    .any(|target| target.kind.iter().any(|kind| kind == "lib"))
        })
        .collect::<Vec<_>>();
    let exclusion_ids = exclusions
        .iter()
        .map(|(exclusion, key)| (key, exclusion.package.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut passed = 0usize;
    let mut excluded = 0usize;

    println!(
        "\nWindows {} sweep ({} packages)",
        sweep.label(),
        packages.len()
    );
    for package in packages {
        let closure = dependency_packages(repo, target, &package.name, sweep)?;
        let roots = closure
            .iter()
            .filter_map(|key| exclusion_ids.get(key).copied())
            .collect::<BTreeSet<_>>();
        if !roots.is_empty() {
            excluded += 1;
            println!(
                "EXCLUDED\t{}\t{}",
                package.name,
                roots.into_iter().collect::<Vec<_>>().join(",")
            );
            continue;
        }
        let output = cargo_check(repo, target, &package.name, sweep)?;
        if !output.status.success() {
            return Err(format!(
                "unexplained Windows {} failure for {}:\n{}",
                sweep.label(),
                package.name,
                output_text(&output)
            ));
        }
        passed += 1;
        println!("PASS\t{}", package.name);
    }
    println!(
        "Windows {} totals: pass={} excluded={} total={}",
        sweep.label(),
        passed,
        excluded,
        passed + excluded
    );
    Ok(())
}

fn cargo_check(repo: &Path, target: &str, package: &str, sweep: Sweep) -> Result<Output, String> {
    let mut command = Command::new("cargo");
    command.args([
        "check",
        "--manifest-path",
        "core/Cargo.toml",
        "-p",
        package,
        "--locked",
        "--offline",
        "--target",
        target,
    ]);
    match sweep {
        Sweep::Library => {
            command.arg("--lib");
        }
        Sweep::Tests => {
            command.arg("--tests");
        }
    }
    command
        .current_dir(repo)
        .env("CARGO_BUILD_JOBS", "1")
        .output()
        .map_err(|error| format!("start Windows cargo check for {package}: {error}"))
}

fn dependency_packages(
    repo: &Path,
    target: &str,
    package: &str,
    sweep: Sweep,
) -> Result<BTreeSet<PackageKey>, String> {
    let edges = if sweep.includes_dev() {
        "normal,build,dev"
    } else {
        "normal,build"
    };
    let output = Command::new("cargo")
        .args([
            "tree",
            "--manifest-path",
            "core/Cargo.toml",
            "-p",
            package,
            "--target",
            target,
            "--locked",
            "--offline",
            "-e",
            edges,
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("start Windows cargo tree for {package}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Windows cargo tree failed for {package}:\n{}",
            output_text(&output)
        ));
    }
    parse_package_tree(&String::from_utf8_lossy(&output.stdout))
}

fn parse_package_tree(text: &str) -> Result<BTreeSet<PackageKey>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields
                .next()
                .ok_or_else(|| format!("cargo tree emitted an empty package row: {line:?}"))?;
            let version = fields
                .next()
                .and_then(|value| value.strip_prefix('v'))
                .ok_or_else(|| format!("cargo tree emitted an invalid package row: {line:?}"))?;
            Ok(PackageKey {
                name: name.to_owned(),
                version: version.to_owned(),
            })
        })
        .collect()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::{PackageKey, parse_package_tree};

    #[test]
    fn package_tree_parser_deduplicates_exact_name_and_version_pairs() {
        assert_eq!(
            parse_package_tree("app v2.0.0 (/repo/app)\nshared v1.2.3\nshared v1.2.3 (*)\n")
                .expect("package tree")
                .into_iter()
                .collect::<Vec<_>>(),
            [
                PackageKey {
                    name: "app".to_owned(),
                    version: "2.0.0".to_owned(),
                },
                PackageKey {
                    name: "shared".to_owned(),
                    version: "1.2.3".to_owned(),
                },
            ]
        );
    }
}
