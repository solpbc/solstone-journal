// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use quote::ToTokens;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use syn::visit::{self, Visit};

pub const HOST_EXCLUDES: &[&str] = &[
    "solstone-core-speakers-analyze",
    "solstone-core-speakers-onnx",
    "solstone-core-vad-analyze",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub version: u32,
    pub sets: Vec<String>,
    pub areas: Vec<String>,
    pub platforms: Vec<String>,
    pub prerequisites: Vec<String>,
    pub serial_groups: Vec<String>,
    pub runtimes: Vec<String>,
    pub timeouts: BTreeMap<String, u64>,
    #[serde(default)]
    pub suites: Vec<Suite>,
    #[serde(default)]
    pub package_suites: Vec<PackageSuite>,
    #[serde(default)]
    pub legs: Vec<Leg>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub id: String,
    pub package: String,
    pub target: String,
    pub set: String,
    pub areas: Vec<String>,
    pub platforms: Vec<String>,
    #[serde(default)]
    pub prerequisites: Vec<String>,
    pub timeout: String,
    #[serde(default)]
    pub serial_group: Option<String>,
    pub default_full: bool,
    #[serde(default)]
    pub required_features: Vec<String>,
    pub runtime: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Leg {
    pub id: String,
    pub make_target: String,
    pub set: String,
    pub areas: Vec<String>,
    pub packages: Vec<String>,
    pub platforms: Vec<String>,
    #[serde(default)]
    pub prerequisites: Vec<String>,
    pub timeout: String,
    #[serde(default)]
    pub serial_group: Option<String>,
    pub default_full: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSuite {
    pub id: String,
    pub package: String,
    pub set: String,
    pub areas: Vec<String>,
    pub platforms: Vec<String>,
    #[serde(default)]
    pub prerequisites: Vec<String>,
    pub timeout: String,
    #[serde(default)]
    pub serial_group: Option<String>,
    pub default_full: bool,
    pub runtime: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CargoSuite {
    pub package: String,
    pub target: String,
    pub required_features: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryBaseline {
    pub version: u32,
    #[serde(default)]
    pub findings: Vec<BoundaryFinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryFinding {
    pub id: String,
}

pub fn load_registry(path: &Path) -> Result<Registry, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read suite registry {}: {error}", path.display()))?;
    toml_edit::de::from_str(&text)
        .map_err(|error| format!("parse suite registry {}: {error}", path.display()))
}

pub fn load_boundary(path: &Path) -> Result<BoundaryBaseline, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read routine boundary {}: {error}", path.display()))?;
    toml_edit::de::from_str(&text)
        .map_err(|error| format!("parse routine boundary {}: {error}", path.display()))
}

pub fn validate_registry(repo: &Path, registry: &Registry) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if registry.version != 1 {
        errors.push(format!(
            "unsupported registry version {}; expected 1",
            registry.version
        ));
    }
    validate_vocabulary("set", &registry.sets, &mut errors);
    validate_vocabulary("area", &registry.areas, &mut errors);
    validate_vocabulary("platform", &registry.platforms, &mut errors);
    validate_vocabulary("prerequisite", &registry.prerequisites, &mut errors);
    validate_vocabulary("serial group", &registry.serial_groups, &mut errors);
    validate_vocabulary("runtime", &registry.runtimes, &mut errors);
    if registry.timeouts.is_empty() {
        errors.push("timeout vocabulary is empty".to_owned());
    }
    for (name, seconds) in &registry.timeouts {
        if name.trim().is_empty() || *seconds == 0 {
            errors.push(format!("invalid timeout class {name:?}={seconds}"));
        }
    }

    let known_sets = registry.sets.iter().cloned().collect::<BTreeSet<_>>();
    let known_areas = registry.areas.iter().cloned().collect::<BTreeSet<_>>();
    let known_platforms = registry.platforms.iter().cloned().collect::<BTreeSet<_>>();
    let known_prereqs = registry
        .prerequisites
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let known_serial_groups = registry
        .serial_groups
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let known_runtimes = registry.runtimes.iter().cloned().collect::<BTreeSet<_>>();
    for required in ["differential", "race", "live"] {
        if !known_sets.contains(required) {
            errors.push(format!(
                "set vocabulary must explicitly include excluded lane {required}"
            ));
        }
    }
    let mut ids = BTreeSet::new();
    let mut registered = BTreeMap::new();

    for suite in &registry.suites {
        validate_entry(
            &suite.id,
            &suite.set,
            &suite.areas,
            &suite.platforms,
            &suite.prerequisites,
            &suite.timeout,
            &known_sets,
            &known_areas,
            &known_platforms,
            &known_prereqs,
            &registry.timeouts,
            &mut errors,
        );
        if !ids.insert(suite.id.clone()) {
            errors.push(format!("duplicate registry id {}", suite.id));
        }
        let expected_id = format!("{}::{}", suite.package, suite.target);
        if suite.id != expected_id {
            errors.push(format!(
                "suite {} id must be package::target ({expected_id})",
                suite.id
            ));
        }
        let key = (suite.package.clone(), suite.target.clone());
        if registered.insert(key.clone(), suite).is_some() {
            errors.push(format!(
                "Cargo integration target {}::{} is registered more than once",
                key.0, key.1
            ));
        }
        let mut features = suite.required_features.clone();
        features.sort();
        features.dedup();
        if features != suite.required_features {
            errors.push(format!(
                "suite {} required_features must be sorted and unique",
                suite.id
            ));
        }
        if suite
            .required_features
            .iter()
            .any(|feature| feature == "differential")
            && (suite.set != "differential" || suite.default_full)
        {
            errors.push(format!(
                "differential suite {} must use set=differential and default_full=false",
                suite.id
            ));
        }
        if let Some(group) = &suite.serial_group
            && !known_serial_groups.contains(group)
        {
            errors.push(format!(
                "suite {} uses unknown serial group {group}",
                suite.id
            ));
        }
        if !known_runtimes.contains(&suite.runtime) {
            errors.push(format!(
                "suite {} uses unknown runtime {}",
                suite.id, suite.runtime
            ));
        }
        validate_default_exclusion(&suite.id, &suite.set, suite.default_full, &mut errors);
    }

    let mut registered_packages = BTreeSet::new();
    for package_suite in &registry.package_suites {
        validate_entry(
            &package_suite.id,
            &package_suite.set,
            &package_suite.areas,
            &package_suite.platforms,
            &package_suite.prerequisites,
            &package_suite.timeout,
            &known_sets,
            &known_areas,
            &known_platforms,
            &known_prereqs,
            &registry.timeouts,
            &mut errors,
        );
        if !ids.insert(package_suite.id.clone()) {
            errors.push(format!("duplicate registry id {}", package_suite.id));
        }
        let expected_id = format!("package::{}", package_suite.package);
        if package_suite.id != expected_id {
            errors.push(format!(
                "package suite {} id must be package::name ({expected_id})",
                package_suite.id
            ));
        }
        if !registered_packages.insert(package_suite.package.clone()) {
            errors.push(format!(
                "workspace package {} has more than one package suite",
                package_suite.package
            ));
        }
        if let Some(group) = &package_suite.serial_group
            && !known_serial_groups.contains(group)
        {
            errors.push(format!(
                "package suite {} uses unknown serial group {group}",
                package_suite.id
            ));
        }
        if !known_runtimes.contains(&package_suite.runtime) {
            errors.push(format!(
                "package suite {} uses unknown runtime {}",
                package_suite.id, package_suite.runtime
            ));
        }
        validate_default_exclusion(
            &package_suite.id,
            &package_suite.set,
            package_suite.default_full,
            &mut errors,
        );
    }

    for leg in &registry.legs {
        validate_entry(
            &leg.id,
            &leg.set,
            &leg.areas,
            &leg.platforms,
            &leg.prerequisites,
            &leg.timeout,
            &known_sets,
            &known_areas,
            &known_platforms,
            &known_prereqs,
            &registry.timeouts,
            &mut errors,
        );
        if !ids.insert(leg.id.clone()) {
            errors.push(format!("duplicate registry id {}", leg.id));
        }
        if leg.make_target.trim().is_empty() {
            errors.push(format!("leg {} has an empty make_target", leg.id));
        }
        if leg.packages.is_empty() {
            errors.push(format!(
                "leg {} must name at least one package selector",
                leg.id
            ));
        }
        if let Some(group) = &leg.serial_group
            && !known_serial_groups.contains(group)
        {
            errors.push(format!("leg {} uses unknown serial group {group}", leg.id));
        }
        validate_default_exclusion(&leg.id, &leg.set, leg.default_full, &mut errors);
    }

    match discover_workspace_packages(repo) {
        Ok(packages) => {
            for package in packages.difference(&registered_packages) {
                errors.push(format!("workspace package {package} has no package suite"));
            }
            for package in registered_packages.difference(&packages) {
                errors.push(format!("stale or unknown package suite {package}"));
            }
            let mut package_selectors = packages;
            package_selectors.insert("workspace".to_owned());
            for leg in &registry.legs {
                for package in &leg.packages {
                    if !package_selectors.contains(package) {
                        errors.push(format!(
                            "leg {} uses unknown package selector {package}",
                            leg.id
                        ));
                    }
                }
            }
        }
        Err(error) => errors.push(error),
    }

    if let Err(error) = validate_host_excludes(repo) {
        errors.push(error);
    }

    match discover_integration_suites(repo) {
        Ok(discovered) => {
            let discovered_map = discovered
                .into_iter()
                .map(|suite| ((suite.package.clone(), suite.target.clone()), suite))
                .collect::<BTreeMap<_, _>>();
            for (key, discovered_suite) in &discovered_map {
                match registered.get(key) {
                    None => errors.push(format!(
                        "unregistered Cargo integration target {}::{}",
                        key.0, key.1
                    )),
                    Some(registered_suite) => {
                        if registered_suite.required_features != discovered_suite.required_features
                        {
                            errors.push(format!(
                                "suite {} required_features {:?} do not match Cargo manifest {:?}",
                                registered_suite.id,
                                registered_suite.required_features,
                                discovered_suite.required_features
                            ));
                        }
                    }
                }
            }
            for key in registered.keys() {
                if !discovered_map.contains_key(key) {
                    errors.push(format!(
                        "stale or unknown Cargo integration target {}::{}",
                        key.0, key.1
                    ));
                }
            }
        }
        Err(error) => errors.push(error),
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_entry(
    id: &str,
    set: &str,
    areas: &[String],
    platforms: &[String],
    prerequisites: &[String],
    timeout: &str,
    known_sets: &BTreeSet<String>,
    known_areas: &BTreeSet<String>,
    known_platforms: &BTreeSet<String>,
    known_prereqs: &BTreeSet<String>,
    timeouts: &BTreeMap<String, u64>,
    errors: &mut Vec<String>,
) {
    if id.trim().is_empty() {
        errors.push("registry entry has an empty id".to_owned());
    }
    if !known_sets.contains(set) {
        errors.push(format!("entry {id} uses unknown set {set}"));
    }
    validate_values(id, "area", areas, known_areas, errors);
    validate_values(id, "platform", platforms, known_platforms, errors);
    validate_values(id, "prerequisite", prerequisites, known_prereqs, errors);
    if !timeouts.contains_key(timeout) {
        errors.push(format!("entry {id} uses unknown timeout class {timeout}"));
    }
}

fn validate_values(
    id: &str,
    kind: &str,
    values: &[String],
    known: &BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    if values.is_empty() && kind != "prerequisite" {
        errors.push(format!("entry {id} has no {kind} values"));
    }
    let mut observed = BTreeSet::new();
    for value in values {
        if !known.contains(value) {
            errors.push(format!("entry {id} uses unknown {kind} {value}"));
        }
        if !observed.insert(value) {
            errors.push(format!("entry {id} repeats {kind} {value}"));
        }
    }
}

fn validate_vocabulary(kind: &str, values: &[String], errors: &mut Vec<String>) {
    let mut observed = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            errors.push(format!("{kind} vocabulary contains an empty value"));
        }
        if !observed.insert(value) {
            errors.push(format!("{kind} vocabulary repeats {value}"));
        }
    }
}

fn validate_default_exclusion(id: &str, set: &str, default_full: bool, errors: &mut Vec<String>) {
    if matches!(set, "differential" | "race" | "live") && default_full {
        errors.push(format!(
            "entry {id} in excluded set {set} cannot be default_full"
        ));
    }
    if set == "live" {
        errors.push(format!(
            "entry {id} attempts to automate the live lane; live validation is operator-only"
        ));
    }
}

pub fn discover_integration_suites(repo: &Path) -> Result<Vec<CargoSuite>, String> {
    let workspace_path = repo.join("core/Cargo.toml");
    let workspace = parse_toml(&workspace_path)?;
    let members = workspace
        .get("workspace")
        .and_then(|item| item.get("members"))
        .and_then(toml_edit::Item::as_array)
        .ok_or_else(|| {
            format!(
                "{} has no workspace.members array",
                workspace_path.display()
            )
        })?;
    let mut suites = BTreeSet::new();

    for member in members.iter() {
        let member = member
            .as_str()
            .ok_or_else(|| "workspace member is not a string".to_owned())?;
        if member.contains('*') || member.contains('?') || member.contains('[') {
            return Err(format!(
                "workspace member {member:?} is a glob; CI discovery requires explicit members"
            ));
        }
        let member_root = repo.join("core").join(member);
        let manifest_path = member_root.join("Cargo.toml");
        let manifest = parse_toml(&manifest_path)?;
        let package = manifest
            .get("package")
            .and_then(|item| item.get("name"))
            .and_then(toml_edit::Item::as_str)
            .ok_or_else(|| format!("{} has no package.name", manifest_path.display()))?
            .to_owned();
        let autotests = manifest
            .get("package")
            .and_then(|item| item.get("autotests"))
            .and_then(toml_edit::Item::as_bool)
            .unwrap_or(true);

        let mut explicit_names = BTreeSet::new();
        if let Some(tests) = manifest
            .get("test")
            .and_then(toml_edit::Item::as_array_of_tables)
        {
            for test in tests {
                let name = test
                    .get("name")
                    .and_then(toml_edit::Item::as_str)
                    .ok_or_else(|| {
                        format!("{} has [[test]] without name", manifest_path.display())
                    })?
                    .to_owned();
                let mut required_features = test
                    .get("required-features")
                    .and_then(toml_edit::Item::as_array)
                    .map(|array| {
                        array
                            .iter()
                            .filter_map(toml_edit::Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                required_features.sort();
                required_features.dedup();
                explicit_names.insert(name.clone());
                suites.insert(CargoSuite {
                    package: package.clone(),
                    target: name,
                    required_features,
                });
            }
        }

        if autotests {
            for name in automatic_test_names(&member_root.join("tests"))? {
                if !explicit_names.contains(&name) {
                    suites.insert(CargoSuite {
                        package: package.clone(),
                        target: name,
                        required_features: Vec::new(),
                    });
                }
            }
        }
    }

    Ok(suites.into_iter().collect())
}

fn discover_workspace_packages(repo: &Path) -> Result<BTreeSet<String>, String> {
    let workspace_path = repo.join("core/Cargo.toml");
    let workspace = parse_toml(&workspace_path)?;
    let members = workspace
        .get("workspace")
        .and_then(|item| item.get("members"))
        .and_then(toml_edit::Item::as_array)
        .ok_or_else(|| {
            format!(
                "{} has no workspace.members array",
                workspace_path.display()
            )
        })?;
    let mut packages = BTreeSet::new();
    for member in members.iter() {
        let member = member
            .as_str()
            .ok_or_else(|| "workspace member is not a string".to_owned())?;
        let manifest_path = repo.join("core").join(member).join("Cargo.toml");
        let manifest = parse_toml(&manifest_path)?;
        let package = manifest
            .get("package")
            .and_then(|item| item.get("name"))
            .and_then(toml_edit::Item::as_str)
            .ok_or_else(|| format!("{} has no package.name", manifest_path.display()))?;
        if !packages.insert(package.to_owned()) {
            return Err(format!("workspace repeats package name {package}"));
        }
    }
    Ok(packages)
}

fn validate_host_excludes(repo: &Path) -> Result<(), String> {
    let makefile = fs::read_to_string(repo.join("Makefile"))
        .map_err(|error| format!("read Makefile for RUST_HOST_EXCLUDES: {error}"))?;
    let declaration = makefile
        .lines()
        .find_map(|line| line.strip_prefix("RUST_HOST_EXCLUDES := "))
        .ok_or_else(|| "Makefile has no exact RUST_HOST_EXCLUDES declaration".to_owned())?;
    let words = declaration.split_whitespace().collect::<Vec<_>>();
    if words.len() % 2 != 0 || words.chunks_exact(2).any(|pair| pair[0] != "--exclude") {
        return Err("RUST_HOST_EXCLUDES must contain --exclude PACKAGE pairs".to_owned());
    }
    let observed = words
        .chunks_exact(2)
        .map(|pair| pair[1])
        .collect::<BTreeSet<_>>();
    let expected = HOST_EXCLUDES.iter().copied().collect::<BTreeSet<_>>();
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "RUST_HOST_EXCLUDES {:?} does not match the routine scanner excludes {:?}",
            observed, expected
        ))
    }
}

fn parse_toml(path: &Path) -> Result<toml_edit::DocumentMut, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read manifest {}: {error}", path.display()))?;
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("parse manifest {}: {error}", path.display()))
}

fn automatic_test_names(tests_dir: &Path) -> Result<Vec<String>, String> {
    let entries = match fs::read_dir(tests_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", tests_dir.display())),
    };
    let mut names = BTreeSet::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("read {} entry: {error}", tests_dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                names.insert(stem.to_owned());
            }
        } else if path.is_dir()
            && path.join("main.rs").is_file()
            && let Some(name) = path.file_name().and_then(|value| value.to_str())
        {
            names.insert(name.to_owned());
        }
    }
    Ok(names.into_iter().collect())
}

pub fn scan_routine_boundaries(repo: &Path) -> Result<BTreeSet<String>, String> {
    let workspace_path = repo.join("core/Cargo.toml");
    let workspace = parse_toml(&workspace_path)?;
    let members = workspace
        .get("workspace")
        .and_then(|item| item.get("members"))
        .and_then(toml_edit::Item::as_array)
        .ok_or_else(|| {
            format!(
                "{} has no workspace.members array",
                workspace_path.display()
            )
        })?;
    let mut findings = BTreeSet::new();

    for member in members.iter() {
        let member = member
            .as_str()
            .ok_or_else(|| "workspace member is not a string".to_owned())?;
        let member_root = repo.join("core").join(member);
        let manifest = parse_toml(&member_root.join("Cargo.toml"))?;
        let package = manifest
            .get("package")
            .and_then(|item| item.get("name"))
            .and_then(toml_edit::Item::as_str)
            .ok_or_else(|| format!("{member} has no package.name"))?;
        if HOST_EXCLUDES.contains(&package) {
            continue;
        }
        let src = member_root.join("src");
        for file in rust_files(&src)? {
            let relative = file
                .strip_prefix(repo)
                .map_err(|error| format!("strip {}: {error}", file.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            let text = fs::read_to_string(&file)
                .map_err(|error| format!("read Rust source {}: {error}", file.display()))?;
            let syntax = syn::parse_file(&text)
                .map_err(|error| format!("parse Rust source {}: {error}", file.display()))?;
            let file_is_test = file.components().any(|part| part.as_os_str() == "tests")
                || file
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name == "tests" || name.ends_with("_tests"));
            let mut visitor = RiskVisitor {
                package,
                relative: &relative,
                test_scope: file_is_test,
                module_path: Vec::new(),
                process_command_aliases: vec![BTreeSet::new()],
                findings: &mut findings,
            };
            visitor.visit_file(&syntax);
        }
    }
    Ok(findings)
}

pub fn validate_boundary(repo: &Path, baseline: &BoundaryBaseline) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if baseline.version != 1 {
        errors.push(format!(
            "unsupported routine boundary version {}; expected 1",
            baseline.version
        ));
    }
    let expected = baseline
        .findings
        .iter()
        .map(|finding| finding.id.clone())
        .collect::<BTreeSet<_>>();
    if expected.len() != baseline.findings.len() {
        errors.push("routine boundary contains duplicate findings".to_owned());
    }
    match scan_routine_boundaries(repo) {
        Ok(observed) => {
            for finding in observed.difference(&expected) {
                errors.push(format!("new routine-boundary risk: {finding}"));
            }
            for finding in expected.difference(&observed) {
                errors.push(format!(
                    "stale routine-boundary risk (shrink the baseline deliberately): {finding}"
                ));
            }
        }
        Err(error) => errors.push(error),
    }
    if let Err(error) = validate_boundary_history(repo, &expected) {
        errors.push(error);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_boundary_history(repo: &Path, current: &BTreeSet<String>) -> Result<(), String> {
    if !repo.join(".git").exists() {
        return Ok(());
    }
    let revisions = git_boundary_revisions(repo)?;
    for revision in &revisions {
        let boundary = git_boundary(repo, revision)?.ok_or_else(|| {
            format!("commit {revision} changed the routine boundary but does not contain it")
        })?;
        for parent in git_parents(repo, revision)? {
            let Some(parent_boundary) = git_boundary(repo, &parent)? else {
                continue;
            };
            let added = boundary_additions(&parent_boundary, &boundary);
            if !added.is_empty() {
                return Err(format_boundary_growth(
                    &format!("{revision} relative to parent {parent}"),
                    &added,
                ));
            }
        }
    }
    if let Some(tracked) = git_boundary(repo, "HEAD")?
        && tracked != *current
    {
        let added = boundary_additions(&tracked, current);
        if !added.is_empty() {
            return Err(format_boundary_growth("HEAD", &added));
        }
    }
    Ok(())
}

fn boundary_additions(previous: &BTreeSet<String>, current: &BTreeSet<String>) -> Vec<String> {
    current.difference(previous).cloned().collect()
}

fn format_boundary_growth(source: &str, added: &[String]) -> String {
    format!(
        "routine boundary may only shrink; {} finding(s) were added at {source}: {}",
        added.len(),
        added.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
    )
}

fn git_boundary_revisions(repo: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .args([
            "log",
            "--full-history",
            "--format=%H",
            "--",
            "core/ci/routine-boundaries.toml",
        ])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("start git for routine-boundary log: {error}"))?;
    if !output.status.success() {
        return Err("git log failed for routine-boundary history".to_owned());
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("decode routine-boundary log: {error}"))
        .map(|text| text.lines().map(ToOwned::to_owned).collect())
}

fn git_parents(repo: &Path, revision: &str) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .args(["show", "-s", "--format=%P", revision])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("start git for parents of {revision}: {error}"))?;
    if !output.status.success() {
        return Err(format!("git could not read parents of {revision}"));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("decode parents of {revision}: {error}"))
        .map(|text| text.split_whitespace().map(ToOwned::to_owned).collect())
}

fn git_boundary(repo: &Path, revision: &str) -> Result<Option<BTreeSet<String>>, String> {
    let output = Command::new("git")
        .args([
            "show",
            &format!("{revision}:core/ci/routine-boundaries.toml"),
        ])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("start git for routine-boundary history: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|error| format!("decode historical routine boundary: {error}"))?;
    let baseline: BoundaryBaseline = toml_edit::de::from_str(&text)
        .map_err(|error| format!("parse {revision} routine boundary: {error}"))?;
    Ok(Some(
        baseline
            .findings
            .into_iter()
            .map(|finding| finding.id)
            .collect(),
    ))
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("read {}: {error}", directory.display()))?
        {
            let path = entry
                .map_err(|error| format!("read {} entry: {error}", directory.display()))?
                .path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

struct RiskVisitor<'a> {
    package: &'a str,
    relative: &'a str,
    test_scope: bool,
    module_path: Vec<String>,
    process_command_aliases: Vec<BTreeSet<String>>,
    findings: &'a mut BTreeSet<String>,
}

impl RiskVisitor<'_> {
    fn inspect(&mut self, name: &str, block: &syn::Block) {
        let scope = if self.module_path.is_empty() {
            name.to_owned()
        } else {
            format!("{}::{name}", self.module_path.join("::"))
        };
        let tokens = block.to_token_stream().to_string();
        let calls = inspect_calls(block, &self.process_command_aliases);
        for (category, needles) in risk_patterns() {
            let found = match *category {
                "clock" => {
                    calls
                        .names
                        .iter()
                        .any(|name| matches!(name.as_str(), "sleep" | "timeout" | "interval"))
                        || needles.iter().any(|needle| tokens.contains(needle))
                }
                "scheduling" => {
                    calls
                        .names
                        .iter()
                        .any(|name| matches!(name.as_str(), "spawn" | "channel"))
                        || needles.iter().any(|needle| tokens.contains(needle))
                }
                "process" => calls.launches_process,
                "native" => calls.reaches_native,
                "host-tool" => {
                    calls.launches_process && needles.iter().any(|needle| tokens.contains(needle))
                }
                _ => needles.iter().any(|needle| tokens.contains(needle)),
            };
            if found {
                self.findings.insert(format!(
                    "{}::{}::{}::{}",
                    self.package, self.relative, scope, category
                ));
            }
        }
    }
}

impl<'ast> Visit<'ast> for RiskVisitor<'_> {
    fn visit_file(&mut self, node: &'ast syn::File) {
        collect_item_process_aliases(
            &node.items,
            self.process_command_aliases
                .last_mut()
                .expect("root process command alias scope"),
        );
        visit::visit_file(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let prior = self.test_scope;
        self.test_scope = prior || has_test_cfg(&node.attrs) || node.ident == "tests";
        self.module_path.push(node.ident.to_string());
        let parent_aliases = self
            .process_command_aliases
            .last()
            .expect("parent process command alias scope")
            .clone();
        let mut aliases = BTreeSet::new();
        if let Some((_, items)) = &node.content {
            collect_item_process_aliases(items, &mut aliases);
            collect_parent_process_aliases(items, &parent_aliases, &mut aliases);
        }
        self.process_command_aliases.push(aliases);
        visit::visit_item_mod(self, node);
        self.process_command_aliases.pop();
        self.module_path.pop();
        self.test_scope = prior;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let is_test = self.test_scope || has_test_attr(&node.attrs);
        if is_test {
            self.inspect(&node.sig.ident.to_string(), &node.block);
        }
        let prior = self.test_scope;
        self.test_scope = is_test;
        visit::visit_item_fn(self, node);
        self.test_scope = prior;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if self.test_scope || has_test_attr(&node.attrs) {
            self.inspect(&node.sig.ident.to_string(), &node.block);
        }
        visit::visit_impl_item_fn(self, node);
    }
}

#[derive(Default)]
struct CallRisks {
    names: BTreeSet<String>,
    launches_process: bool,
    reaches_native: bool,
}

fn inspect_calls(block: &syn::Block, module_aliases: &[BTreeSet<String>]) -> CallRisks {
    let mut visitor = CallRiskVisitor {
        module_aliases,
        block_aliases: Vec::new(),
        risks: CallRisks::default(),
    };
    visitor.visit_block(block);
    visitor.risks
}

struct CallRiskVisitor<'a> {
    module_aliases: &'a [BTreeSet<String>],
    block_aliases: Vec<BTreeSet<String>>,
    risks: CallRisks,
}

impl<'ast> Visit<'ast> for CallRiskVisitor<'_> {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        let mut aliases = self.block_aliases.last().cloned().unwrap_or_else(|| {
            self.module_aliases
                .last()
                .expect("function module alias scope")
                .clone()
        });
        collect_block_process_aliases(node, &mut aliases);
        self.block_aliases.push(aliases);
        visit::visit_block(self, node);
        self.block_aliases.pop();
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(function) = node.func.as_ref() {
            if let Some(segment) = function.path.segments.last() {
                self.risks.names.insert(segment.ident.to_string());
            }
            let segments = function
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>();
            let launches_process = is_process_constructor(
                &segments,
                self.block_aliases.last().expect("call block alias scope"),
                self.module_aliases,
            );
            self.risks.launches_process |= launches_process;
            self.risks.reaches_native |= segments
                .iter()
                .any(|segment| is_native_call_segment(segment));
            if launches_process {
                self.risks.reaches_native |= node
                    .args
                    .first()
                    .and_then(static_string)
                    .is_some_and(|program| is_native_executable(&program));
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.risks.names.insert(node.method.to_string());
        visit::visit_expr_method_call(self, node);
    }
}

fn static_string(expression: &syn::Expr) -> Option<String> {
    if let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(value),
        ..
    }) = expression
    {
        Some(value.value())
    } else {
        None
    }
}

fn is_native_call_segment(segment: &str) -> bool {
    matches!(
        segment,
        "ffmpeg" | "ffmpeg_next" | "libloading" | "pdfium" | "onnx" | "vulkan" | "maturin"
    )
}

fn is_native_executable(program: &str) -> bool {
    program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| matches!(name, "ffmpeg" | "maturin"))
}

fn is_process_constructor(
    segments: &[String],
    local_aliases: &BTreeSet<String>,
    module_aliases: &[BTreeSet<String>],
) -> bool {
    if segments.last().map(String::as_str) != Some("new") {
        return false;
    }
    let direct = path_ends_with(segments, &["std", "process", "Command", "new"])
        || path_ends_with(segments, &["tokio", "process", "Command", "new"]);
    if direct {
        return true;
    }
    let Some((alias, qualifiers)) = segments[..segments.len() - 1].split_last() else {
        return false;
    };
    if qualifiers.is_empty() {
        return local_aliases.contains(alias);
    }
    let target_scope = if qualifiers == ["self"] {
        module_aliases.last()
    } else if qualifiers == ["crate"] {
        module_aliases.first()
    } else if qualifiers.iter().all(|segment| segment == "super")
        && qualifiers.len() < module_aliases.len()
    {
        module_aliases.get(module_aliases.len() - 1 - qualifiers.len())
    } else {
        None
    };
    target_scope.is_some_and(|aliases| aliases.contains(alias))
}

fn path_ends_with(segments: &[String], suffix: &[&str]) -> bool {
    segments.len() >= suffix.len()
        && segments[segments.len() - suffix.len()..]
            .iter()
            .map(String::as_str)
            .eq(suffix.iter().copied())
}

fn collect_item_process_aliases(items: &[syn::Item], aliases: &mut BTreeSet<String>) {
    for item in items {
        if let syn::Item::Use(item_use) = item {
            collect_process_aliases(&item_use.tree, &mut Vec::new(), aliases);
        }
    }
}

fn collect_block_process_aliases(block: &syn::Block, aliases: &mut BTreeSet<String>) {
    for statement in &block.stmts {
        if let syn::Stmt::Item(syn::Item::Use(item_use)) = statement {
            collect_process_aliases(&item_use.tree, &mut Vec::new(), aliases);
        }
    }
}

fn collect_parent_process_aliases(
    items: &[syn::Item],
    parent_aliases: &BTreeSet<String>,
    aliases: &mut BTreeSet<String>,
) {
    for item in items {
        if let syn::Item::Use(item_use) = item {
            collect_relative_process_aliases(
                &item_use.tree,
                &mut Vec::new(),
                parent_aliases,
                aliases,
            );
        }
    }
}

fn collect_relative_process_aliases(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    parent_aliases: &BTreeSet<String>,
    aliases: &mut BTreeSet<String>,
) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_relative_process_aliases(&path.tree, prefix, parent_aliases, aliases);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let name = name.ident.to_string();
            if prefix.as_slice() == ["super"] && parent_aliases.contains(&name) {
                aliases.insert(name);
            }
        }
        syn::UseTree::Rename(rename) => {
            if prefix.as_slice() == ["super"] && parent_aliases.contains(&rename.ident.to_string())
            {
                aliases.insert(rename.rename.to_string());
            }
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_relative_process_aliases(item, prefix, parent_aliases, aliases);
            }
        }
        syn::UseTree::Glob(_) => {
            if prefix.as_slice() == ["super"] {
                aliases.extend(parent_aliases.iter().cloned());
            }
        }
    }
}

fn collect_process_aliases(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    aliases: &mut BTreeSet<String>,
) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_process_aliases(&path.tree, prefix, aliases);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            if is_process_command_path(prefix, &name.ident.to_string()) {
                aliases.insert(name.ident.to_string());
            }
        }
        syn::UseTree::Rename(rename) => {
            if is_process_command_path(prefix, &rename.ident.to_string()) {
                aliases.insert(rename.rename.to_string());
            }
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_process_aliases(item, prefix, aliases);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

fn is_process_command_path(prefix: &[String], name: &str) -> bool {
    name == "Command"
        && (path_ends_with(prefix, &["std", "process"])
            || path_ends_with(prefix, &["tokio", "process"]))
}

fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        path.is_ident("test")
            || path.segments.last().is_some_and(|segment| {
                matches!(
                    segment.ident.to_string().as_str(),
                    "test" | "rstest" | "tokio_test"
                )
            })
    })
}

fn has_test_cfg(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg") && attr.meta.to_token_stream().to_string().contains("test")
    })
}

fn risk_patterns() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("clock", &["Instant", "SystemTime"]),
        ("scheduling", &["Barrier", "Mutex", "RwLock", "select !"]),
        (
            "network",
            &["TcpListener", "TcpStream", "UdpSocket", "reqwest", "ureq"],
        ),
        ("process", &[]),
        ("native", &[]),
        (
            "host-tool",
            &[
                "\"cargo\"",
                "\"make\"",
                "\"git\"",
                "\"curl\"",
                "\"python",
                "\"cc\"",
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Registry) {
        let temp = TempDir::new().expect("tempdir");
        fs::create_dir_all(temp.path().join("core/crates/a/tests")).expect("test dir");
        fs::create_dir_all(temp.path().join("core/crates/a/src")).expect("src dir");
        fs::write(
            temp.path().join("core/Cargo.toml"),
            "[workspace]\nmembers = [\"crates/a\"]\n",
        )
        .expect("workspace");
        fs::write(
            temp.path().join("Makefile"),
            "RUST_HOST_EXCLUDES := --exclude solstone-core-speakers-analyze --exclude solstone-core-speakers-onnx --exclude solstone-core-vad-analyze\n",
        )
        .expect("Makefile");
        fs::write(
            temp.path().join("core/crates/a/Cargo.toml"),
            "[package]\nname = \"a\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("manifest");
        fs::write(
            temp.path().join("core/crates/a/tests/api.rs"),
            "fn main() {}\n",
        )
        .expect("integration test");
        fs::write(
            temp.path().join("core/crates/a/src/lib.rs"),
            "#[cfg(test)] mod tests { #[test] fn pure() { assert_eq!(2 + 2, 4); } }\n",
        )
        .expect("lib");
        let registry = Registry {
            version: 1,
            sets: vec![
                "component".to_owned(),
                "differential".to_owned(),
                "race".to_owned(),
                "live".to_owned(),
            ],
            areas: vec!["a".to_owned()],
            platforms: vec!["linux".to_owned()],
            prerequisites: vec!["cargo-cache".to_owned()],
            serial_groups: vec!["local-services".to_owned()],
            runtimes: vec!["none".to_owned()],
            timeouts: BTreeMap::from([("quick".to_owned(), 30)]),
            suites: vec![Suite {
                id: "a::api".to_owned(),
                package: "a".to_owned(),
                target: "api".to_owned(),
                set: "component".to_owned(),
                areas: vec!["a".to_owned()],
                platforms: vec!["linux".to_owned()],
                prerequisites: vec!["cargo-cache".to_owned()],
                timeout: "quick".to_owned(),
                serial_group: None,
                default_full: true,
                required_features: Vec::new(),
                runtime: "none".to_owned(),
            }],
            package_suites: vec![PackageSuite {
                id: "package::a".to_owned(),
                package: "a".to_owned(),
                set: "component".to_owned(),
                areas: vec!["a".to_owned()],
                platforms: vec!["linux".to_owned()],
                prerequisites: vec!["cargo-cache".to_owned()],
                timeout: "quick".to_owned(),
                serial_group: None,
                default_full: false,
                runtime: "none".to_owned(),
            }],
            legs: Vec::new(),
        };
        (temp, registry)
    }

    #[test]
    fn registry_matches_every_manifest_target_exactly_once() {
        let (temp, registry) = fixture();
        assert_eq!(validate_registry(temp.path(), &registry), Ok(()));
    }

    #[test]
    fn registry_rejects_missing_duplicate_stale_and_unknown_values() {
        let (temp, mut registry) = fixture();
        registry.suites[0].set = "mystery".to_owned();
        registry.suites.push(registry.suites[0].clone());
        registry.suites.push(Suite {
            id: "a::stale".to_owned(),
            target: "stale".to_owned(),
            ..registry.suites[0].clone()
        });
        let errors = validate_registry(temp.path(), &registry).expect_err("must reject mutation");
        let joined = errors.join("\n");
        assert!(joined.contains("unknown set"));
        assert!(joined.contains("registered more than once"));
        assert!(joined.contains("stale or unknown"));

        registry.suites.clear();
        let errors = validate_registry(temp.path(), &registry).expect_err("must reject missing");
        assert!(
            errors
                .join("\n")
                .contains("unregistered Cargo integration target")
        );
    }

    #[test]
    fn registry_enforces_runtime_and_explicit_default_exclusions() {
        let (temp, mut registry) = fixture();
        registry.suites[0].runtime = "ambient".to_owned();
        registry.suites[0].set = "race".to_owned();
        let errors = validate_registry(temp.path(), &registry).expect_err("must reject mutation");
        let joined = errors.join("\n");
        assert!(joined.contains("unknown runtime ambient"));
        assert!(joined.contains("excluded set race cannot be default_full"));

        registry.suites[0].runtime = "none".to_owned();
        registry.suites[0].set = "live".to_owned();
        registry.suites[0].default_full = false;
        let errors = validate_registry(temp.path(), &registry).expect_err("live is operator-only");
        assert!(
            errors
                .join("\n")
                .contains("live validation is operator-only")
        );
    }

    #[test]
    fn routine_boundary_detects_new_risky_test_and_only_shrinks_deliberately() {
        let (temp, _registry) = fixture();
        let clean = BoundaryBaseline {
            version: 1,
            findings: Vec::new(),
        };
        fs::write(
            temp.path().join("core/crates/a/src/lib.rs"),
            r#"use std::process::Command;
            #[cfg(test)] mod tests {
                enum Command { Version }
                struct DomainCommand;
                impl DomainCommand { fn new() -> Self { Self } }
                struct NativePlan { vulkan_identity: &'static str }
                fn stage_onnx_fixture() {}
                fn temporary_path() { let _ = std::process::id(); }
                #[test] fn lexical_lookalikes_are_safe() {
                    let command = Command::Version;
                    let _domain = DomainCommand::new();
                    let spawner = "fake";
                    let sleepy = "fake";
                    let tool = "cargo";
                    let model = "model.onnx";
                    let plan = NativePlan { vulkan_identity: "vulkan" };
                    stage_onnx_fixture();
                    temporary_path();
                    assert!(matches!(command, Command::Version));
                    assert_eq!((spawner, sleepy, tool), ("fake", "fake", "cargo"));
                    assert_eq!((model, plan.vulkan_identity), ("model.onnx", "vulkan"));
                }
                #[test] fn nested_process_alias_does_not_leak() {
                    {
                        use std::process::Command as Runner;
                        let _ = std::mem::size_of::<Runner>();
                    }
                    struct Runner;
                    impl Runner { fn new() -> Self { Self } }
                    let _domain = Runner::new();
                }
            }
"#,
        )
        .expect("safe domain command");
        assert_eq!(validate_boundary(temp.path(), &clean), Ok(()));
        fs::write(
            temp.path().join("core/crates/a/src/lib.rs"),
            r#"use std::process::Command as ParentRunner;
            #[cfg(test)] mod tests {
                use super::*;
                fn timeout<T>() {}
                fn interval<T>() {}
                fn sleep<T>() {}
                fn spawn<T>() {}
                fn ffmpeg(_: &[&str]) {}

                use std::process::Command as Runner;
                use tokio::process::Command as AsyncRunner;

                #[test] fn launches_host_through_explicit_parent_glob() {
                    ParentRunner::new("cargo");
                }
                #[test] fn launches_host_through_qualified_parent() {
                    super::ParentRunner::new("cargo");
                }
                #[test] fn launches_host_through_qualified_crate_root() {
                    crate::ParentRunner::new("cargo");
                }
                #[test] fn launches_host_through_module_alias() {
                    Runner::new("cargo");
                }
                #[test] fn launches_host_through_qualified_self() {
                    self::Runner::new("cargo");
                }
                #[test] fn launches_host_through_local_alias() {
                    use std::process::Command as LocalRunner;
                    LocalRunner::new("cargo");
                }
                #[test] fn launches_host_through_grouped_alias() {
                    use std::process::{Command as GroupedRunner};
                    GroupedRunner::new("cargo");
                }
                #[test] fn launches_host_through_async_alias() {
                    AsyncRunner::new("cargo");
                }
                #[test] fn launches_host_through_direct_path() {
                    std::process::Command::new("cargo");
                }
                #[test] fn launches_native_executable() {
                    Runner::new("ffmpeg");
                }
                #[test] fn non_native_executable_with_native_strings() {
                    Runner::new("echo");
                    let _ = ("model.onnx", "vulkan", "pdfium");
                }
                #[test] fn calls_native_helper() {
                    ffmpeg(&[]);
                }
                #[test] fn calls_native_crate() {
                    ffmpeg_next::init();
                }
                #[test] fn calls_dynamic_loader() {
                    libloading::Library::new("libexample.so");
                }
                #[test] fn opens_turbofished_channel() {
                    let _ = std::sync::mpsc::channel::<()>();
                }
                #[test] fn spawns() {
                    let _ = std::thread::spawn(|| {});
                }
                #[test] fn sleeps() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                #[test] fn calls_turbofished_timeout() {
                    timeout::<()>();
                }
                #[test] fn calls_turbofished_interval() {
                    interval::<()>();
                }
                #[test] fn calls_turbofished_sleep() {
                    sleep::<()>();
                }
                #[test] fn calls_turbofished_spawn() {
                    spawn::<()>();
                }
                struct Clock;
                impl Clock { fn sleep(&self) {} }
                #[test] fn calls_sleep_method() {
                    Clock.sleep();
                }
                struct Spawner;
                impl Spawner { fn spawn(&self) {} }
                #[test] fn calls_spawn_method() {
                    Spawner.spawn();
                }
                mod nested {
                    #[test] fn launches_host_through_two_parents() {
                        super::super::ParentRunner::new("cargo");
                    }
                }
            }
"#,
        )
        .expect("mutate lib");
        let errors = validate_boundary(temp.path(), &clean).expect_err("new risk must red");
        assert!(errors.iter().any(|error| error.contains("process")));
        assert!(errors.iter().any(|error| error.contains("host-tool")));
        assert!(errors.iter().any(|error| error.contains("clock")));
        assert!(errors.iter().any(|error| error.contains("scheduling")));
        assert!(errors.iter().any(|error| error.contains("native")));

        let observed = scan_routine_boundaries(temp.path()).expect("scan risky forms");
        for expected in [
            "launches_host_through_explicit_parent_glob::process",
            "launches_host_through_explicit_parent_glob::host-tool",
            "launches_host_through_qualified_parent::process",
            "launches_host_through_qualified_parent::host-tool",
            "launches_host_through_qualified_crate_root::process",
            "launches_host_through_qualified_crate_root::host-tool",
            "launches_host_through_module_alias::process",
            "launches_host_through_module_alias::host-tool",
            "launches_host_through_qualified_self::process",
            "launches_host_through_qualified_self::host-tool",
            "launches_host_through_local_alias::process",
            "launches_host_through_local_alias::host-tool",
            "launches_host_through_grouped_alias::process",
            "launches_host_through_grouped_alias::host-tool",
            "launches_host_through_async_alias::process",
            "launches_host_through_async_alias::host-tool",
            "launches_host_through_direct_path::process",
            "launches_host_through_direct_path::host-tool",
            "launches_native_executable::process",
            "launches_native_executable::native",
            "non_native_executable_with_native_strings::process",
            "calls_native_helper::native",
            "calls_native_crate::native",
            "calls_dynamic_loader::native",
            "opens_turbofished_channel::scheduling",
            "spawns::scheduling",
            "sleeps::clock",
            "calls_turbofished_timeout::clock",
            "calls_turbofished_interval::clock",
            "calls_turbofished_sleep::clock",
            "calls_turbofished_spawn::scheduling",
            "calls_sleep_method::clock",
            "calls_spawn_method::scheduling",
            "nested::launches_host_through_two_parents::process",
            "nested::launches_host_through_two_parents::host-tool",
        ] {
            assert!(
                observed.iter().any(|finding| finding.ends_with(expected)),
                "missing scanner control {expected}"
            );
        }
        assert!(
            !observed
                .iter()
                .any(|finding| finding
                    .ends_with("non_native_executable_with_native_strings::native")),
            "native words outside call/executable position are not runtime evidence"
        );

        let accepted = BoundaryBaseline {
            version: 1,
            findings: observed
                .into_iter()
                .map(|id| BoundaryFinding { id })
                .collect(),
        };
        assert_eq!(validate_boundary(temp.path(), &accepted), Ok(()));
        fs::write(
            temp.path().join("core/crates/a/src/lib.rs"),
            "#[cfg(test)] mod tests { #[test] fn pure_again() {} }\n",
        )
        .expect("shrink lib");
        let errors =
            validate_boundary(temp.path(), &accepted).expect_err("stale baseline must red");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("shrink the baseline deliberately"))
        );
    }

    #[test]
    fn routine_boundary_history_rejects_growth_and_accepts_shrinkage() {
        let previous = BTreeSet::from(["a".to_owned(), "b".to_owned()]);
        let grown = BTreeSet::from(["a".to_owned(), "b".to_owned(), "c".to_owned()]);
        assert_eq!(boundary_additions(&previous, &grown), ["c"]);
        let shrunk = BTreeSet::from(["a".to_owned()]);
        assert!(boundary_additions(&previous, &shrunk).is_empty());

        let sibling = BTreeSet::from(["b".to_owned()]);
        let good_merge = BTreeSet::new();
        assert!(boundary_additions(&shrunk, &good_merge).is_empty());
        assert!(boundary_additions(&sibling, &good_merge).is_empty());

        let bad_merge = BTreeSet::from(["a".to_owned()]);
        assert!(boundary_additions(&shrunk, &bad_merge).is_empty());
        assert_eq!(boundary_additions(&sibling, &bad_merge), ["a"]);
    }
}
