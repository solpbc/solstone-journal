// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ci::{Registry, load_registry};

/// Inlined when the wheel packaging leaves retired. Reads the `[package] name`
/// out of a Cargo manifest; nothing here is wheel-specific.
fn package_name(manifest: &str) -> String {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && let Some(value) = trimmed.strip_prefix("name = ") {
            return value.trim().trim_matches('"').to_owned();
        }
    }
    panic!("manifest has no package name")
}

fn repo_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("workspace checkout root")
        .to_path_buf();
    assert!(
        root.join("Makefile").is_file(),
        "repo root must contain Makefile"
    );
    root
}
fn dependency_keys(manifest: &str) -> BTreeSet<String> {
    let mut in_dependencies = false;
    let mut keys = BTreeSet::new();

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_dependencies = trimmed == "[dependencies]"
                || (trimmed.starts_with("[target.") && trimmed.ends_with(".dependencies]"));
            continue;
        }
        if !in_dependencies || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, _value)) = trimmed.split_once('=') {
            keys.insert(key.trim().to_owned());
        }
    }

    keys
}

fn explicit_binary_names(manifest: &str) -> BTreeSet<String> {
    let mut in_bin = false;
    let mut names = BTreeSet::new();

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_bin = trimmed == "[[bin]]";
            continue;
        }
        if in_bin && let Some(value) = trimmed.strip_prefix("name = ") {
            names.insert(value.trim().trim_matches('"').to_owned());
        }
    }
    names
}

fn makefile_text(root: &Path) -> String {
    fs::read_to_string(root.join("Makefile")).expect("read Makefile")
}

fn ci_registry(root: &Path) -> Registry {
    load_registry(&root.join("core/ci/suites.toml")).expect("load CI suite registry")
}

fn workspace_members(workspace: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut in_members = false;

    for line in workspace.lines() {
        let trimmed = line.trim();
        if trimmed == "members = [" {
            in_members = true;
            continue;
        }
        if in_members && trimmed == "]" {
            break;
        }
        if in_members {
            let value = trimmed.trim_end_matches(',').trim_matches('"');
            if !value.is_empty() {
                members.push(value.to_owned());
            }
        }
    }

    members
}

fn target_body<'a>(makefile: &'a str, target: &str) -> &'a str {
    let marker = format!("\n{target}:");
    let start = makefile
        .find(&marker)
        .map(|offset| offset + 1)
        .or_else(|| makefile.strip_prefix(&format!("{target}:")).map(|_| 0))
        .expect("Makefile target must exist");
    let rest = &makefile[start..];
    let end = rest
        .lines()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| {
            let target_header = !line.is_empty()
                && !line.starts_with(['\t', ' ', '#'])
                && line
                    .split_once(':')
                    .is_some_and(|(_, suffix)| !suffix.trim_start().starts_with('='));
            target_header.then(|| rest.lines().take(index).map(|item| item.len() + 1).sum())
        })
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn every_host_excluded_crate_is_tested_by_a_ci_target() {
    let makefile = makefile_text(&repo_root());

    let excludes = makefile
        .lines()
        .find(|line| line.starts_with("RUST_HOST_EXCLUDES :="))
        .expect("RUST_HOST_EXCLUDES must be defined");
    let excluded = excludes
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter_map(|pair| (pair[0] == "--exclude").then_some(pair[1].to_owned()))
        .collect::<BTreeSet<_>>();
    assert!(
        !excluded.is_empty(),
        "the exclude parser found nothing; it is measuring itself, not the Makefile"
    );

    let packages = makefile
        .lines()
        .find(|line| line.starts_with("RUST_NATIVE_ROUTINE_PACKAGES :="))
        .expect("RUST_NATIVE_ROUTINE_PACKAGES must be defined");
    let tested = packages
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter_map(|pair| (pair[0] == "-p").then_some(pair[1].to_owned()))
        .collect::<BTreeSet<_>>();

    assert_eq!(
        excluded, tested,
        "a crate excluded from the workspace test selection has no gate running its tests"
    );

    let onnx = target_body(&makefile, "check-rust-onnx-test");
    assert!(
        onnx.contains("$(ONNX_HOST_TEST_PACKAGES)"),
        "check-rust-onnx-test must run the excluded-crate package list, not a hand copy"
    );
    let registry = ci_registry(&repo_root());
    let onnx_leg = registry
        .legs
        .iter()
        .find(|leg| leg.make_target == "check-rust-onnx-test")
        .expect("full registry must retain the excluded-crate ONNX leg");
    assert!(onnx_leg.default_full);
    assert_eq!(
        onnx_leg.packages.iter().cloned().collect::<BTreeSet<_>>(),
        tested,
        "the ONNX registry leg must name every host-excluded package"
    );
}

/// The race gate may name only supervisor tests that use supervisor-race's explicit
/// inconclusive outcome. Removing one from the Makefile list must therefore
/// red this source-derived guard rather than silently reducing coverage.
#[test]
fn every_supervisor_race_test_is_named_in_rust_race_gate() {
    let root = repo_root();
    let makefile = makefile_text(&root);
    let registered = makefile
        .lines()
        .find(|line| line.starts_with("RUST_RACE_TEST_TARGETS :="))
        .expect("RUST_RACE_TEST_TARGETS must be defined")
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter_map(|pair| (pair[0] == "--test").then_some(pair[1].to_owned()))
        .collect::<BTreeSet<_>>();
    assert!(
        !registered.is_empty(),
        "the race-target parser found nothing; it is measuring itself, not the Makefile"
    );

    let tests = root.join("core/crates/solstone-core/tests");
    let expected = fs::read_dir(&tests)
        .expect("read solstone-core integration tests")
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?;
            (file_name.starts_with("supervisor_") && file_name.ends_with(".rs")).then_some(path)
        })
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read supervisor integration test")
                .contains("#[path = \"support/await_outcome.rs\"]")
        })
        .map(|path| {
            path.file_stem()
                .expect("supervisor test file stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(
        registered, expected,
        "RUST_RACE_TEST_TARGETS must exactly name supervisor-race supervisor tests"
    );
    assert!(
        target_body(&makefile, "check-rust-race").contains("$(RUST_RACE_TEST_TARGETS)"),
        "check-rust-race must reference RUST_RACE_TEST_TARGETS, not a hand copy"
    );
}

#[test]
fn classified_same_crate_packages_are_routine_and_have_package_specific_full_routes() {
    let makefile = makefile_text(&repo_root());
    let classified = makefile
        .lines()
        .find_map(|line| line.strip_prefix("RUST_CLASSIFIED_FULL_TEST_PACKAGES := "))
        .expect("RUST_CLASSIFIED_FULL_TEST_PACKAGES must be defined")
        .split_whitespace()
        .collect::<BTreeSet<_>>();
    let registry = ci_registry(&repo_root());
    let native = BTreeSet::from([
        "solstone-core-speakers-analyze",
        "solstone-core-speakers-onnx",
        "solstone-core-vad-analyze",
    ]);
    for package in classified {
        let suite = registry
            .package_suites
            .iter()
            .find(|suite| suite.package == package)
            .unwrap_or_else(|| panic!("{package} has no full package suite"));
        assert!(
            !suite.default_full,
            "{package} package suite would duplicate its classified full leg"
        );
        let leg = if native.contains(package) {
            registry
                .legs
                .iter()
                .find(|leg| leg.make_target == "check-rust-onnx-test")
                .expect("native packages have no aggregate full leg")
        } else {
            let expected_target = format!(
                "check-rust-classified-full-tests-{}",
                package
                    .strip_prefix("solstone-core-")
                    .expect("package prefix")
            );
            registry
                .legs
                .iter()
                .find(|leg| leg.make_target == expected_target)
                .unwrap_or_else(|| panic!("{package} has no classified full leg"))
        };
        assert!(
            leg.default_full,
            "{package} classified leg is not default full"
        );
        assert!(
            leg.packages.iter().any(|candidate| candidate == package),
            "{package} is absent from its classified full leg"
        );
    }
}

#[derive(Debug, Eq, PartialEq)]
struct RuntimeSpecContract {
    digest: String,
    links: Vec<String>,
}

fn quoted_values(line: &str) -> Vec<String> {
    line.split('"')
        .enumerate()
        .filter_map(|(index, value)| (index % 2 == 1).then_some(value.to_owned()))
        .collect()
}

fn rust_runtime_specs(text: &str) -> BTreeMap<String, RuntimeSpecContract> {
    let target_start = text
        .find("pub const TARGETS: &[TargetSpec] = &[")
        .expect("onnx_runtime.rs must define TARGETS");
    let mut specs = BTreeMap::new();
    let mut current_key: Option<String> = None;
    let mut current_digest: Option<String> = None;
    let mut current_links = Vec::new();
    let mut reading_links = false;

    for line in text[target_start..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed == "];" {
            break;
        }
        if trimmed.starts_with("key:") {
            if let Some(key) = current_key.take() {
                specs.insert(
                    key,
                    RuntimeSpecContract {
                        digest: current_digest.take().expect("target runtime digest"),
                        links: std::mem::take(&mut current_links),
                    },
                );
            }
            current_key = quoted_values(trimmed).into_iter().next();
            reading_links = false;
            continue;
        }
        if trimmed.starts_with("runtime_sha256:") {
            current_digest = quoted_values(trimmed).into_iter().next();
            continue;
        }
        if trimmed.starts_with("link_names:") {
            reading_links = true;
        }
        if reading_links {
            current_links.extend(quoted_values(trimmed));
            if trimmed.contains(']') {
                reading_links = false;
            }
        }
    }
    if let Some(key) = current_key {
        specs.insert(
            key,
            RuntimeSpecContract {
                digest: current_digest.expect("target runtime digest"),
                links: current_links,
            },
        );
    }
    specs
}

fn make_assignment(makefile: &str, name: &str) -> String {
    let prefixes = [format!("override {name} := "), format!("{name} := ")];
    makefile
        .lines()
        .find_map(|line| prefixes.iter().find_map(|prefix| line.strip_prefix(prefix)))
        .unwrap_or_else(|| panic!("Makefile assignment must exist: {name}"))
        .to_owned()
}

#[test]
fn make_onnx_runtime_mapping_matches_the_staging_source_of_truth() {
    let root = repo_root();
    let makefile = makefile_text(&root);
    let source =
        fs::read_to_string(root.join("core/crates/solstone-core-distribution/src/onnx_runtime.rs"))
            .expect("read runtime staging source");
    let actual = rust_runtime_specs(&source);
    assert_eq!(
        actual.keys().cloned().collect::<Vec<_>>(),
        ["linux-aarch64", "linux-x86_64", "macos-arm64"],
        "parser positive control must see every runtime target"
    );

    for (make_key, script_key) in [
        ("LINUX_X86_64", "linux-x86_64"),
        ("LINUX_AARCH64", "linux-aarch64"),
        ("MACOS_ARM64", "macos-arm64"),
    ] {
        let expected = actual.get(script_key).expect("source target must exist");
        assert_eq!(
            make_assignment(&makefile, &format!("ONNX_RUNTIME_{make_key}_TARGET")),
            script_key
        );
        assert_eq!(
            make_assignment(&makefile, &format!("ONNX_RUNTIME_{make_key}_DIGEST")),
            expected.digest
        );
        assert_eq!(
            make_assignment(&makefile, &format!("ONNX_RUNTIME_{make_key}_LINK_NAMES"))
                .split_whitespace()
                .collect::<Vec<_>>(),
            expected.links,
            "Make link names drifted from onnx_runtime.rs for {script_key}"
        );
    }
}

#[test]
fn runtime_target_parser_ignores_decoys_and_formatting() {
    let fixture = r#"
const DECOY: TargetSpec = TargetSpec {
    key: "decoy",
    runtime_sha256: "wrong",
    link_names: &["wrong"],
};
pub const TARGETS: &[TargetSpec] = &[
    TargetSpec {
        key: "fixture",
        runtime_sha256: "abc",
        link_names: &["one", "two"],
    },
];
"#;
    let parsed = rust_runtime_specs(fixture);
    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed.get("fixture"),
        Some(&RuntimeSpecContract {
            digest: "abc".to_owned(),
            links: vec!["one".to_owned(), "two".to_owned()],
        })
    );
}

#[test]
fn explicit_workspace_binary_artifact_names_are_unique() {
    let crates = repo_root().join("core/crates");
    let mut owners = BTreeMap::<String, Vec<String>>::new();

    for entry in fs::read_dir(crates).expect("read workspace crates") {
        let manifest = entry
            .expect("read workspace crate entry")
            .path()
            .join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = fs::read_to_string(&manifest).expect("read workspace crate manifest");
        for name in explicit_binary_names(&text) {
            owners.entry(name).or_default().push(package_name(&text));
        }
    }

    let duplicates = owners
        .into_iter()
        .filter(|(_name, packages)| packages.len() > 1)
        .collect::<BTreeMap<_, _>>();
    assert!(
        duplicates.is_empty(),
        "workspace packages must not race to write the same binary artifact: {duplicates:?}"
    );
}

#[test]
fn rust_host_excludes_match_the_workspace_onnx_closure() {
    let root = repo_root();
    let core = root.join("core");
    let members = workspace_members(
        &fs::read_to_string(core.join("Cargo.toml")).expect("read workspace manifest"),
    );
    assert!(!members.is_empty(), "workspace members must not be empty");

    let manifests = members
        .iter()
        .map(|member| {
            let name = Path::new(member)
                .file_name()
                .expect("workspace member must have a crate name")
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(core.join(member).join("Cargo.toml"))
                .expect("read member manifest");
            (name, dependency_keys(&text))
        })
        .collect::<Vec<_>>();

    let mut expected = manifests
        .iter()
        .filter(|(_name, dependencies)| {
            dependencies.contains("ort") || dependencies.contains("ort.workspace")
        })
        .map(|(name, _dependencies)| name.clone())
        .collect::<BTreeSet<_>>();
    loop {
        let additions = manifests
            .iter()
            .filter(|(name, dependencies)| {
                !expected.contains(name)
                    && dependencies.iter().any(|dependency| {
                        expected.contains(dependency.trim_end_matches(".workspace"))
                    })
            })
            .map(|(name, _dependencies)| name.clone())
            .collect::<BTreeSet<_>>();
        if additions.is_empty() {
            break;
        }
        expected.extend(additions);
    }

    assert!(
        !expected.is_empty(),
        "the ONNX closure parser found nothing; it is measuring itself, not the manifests"
    );

    let makefile = fs::read_to_string(root.join("Makefile")).expect("read Makefile");
    let excludes = makefile
        .lines()
        .find_map(|line| line.strip_prefix("RUST_HOST_EXCLUDES := "))
        .expect("RUST_HOST_EXCLUDES must be defined")
        .split_whitespace()
        .collect::<Vec<_>>();
    let actual = excludes
        .chunks_exact(2)
        .map(|pair| {
            assert_eq!(pair[0], "--exclude", "host exclusion must use --exclude");
            pair[1].to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        excludes.len() % 2,
        0,
        "host exclusions must be flag/name pairs"
    );
    assert_eq!(
        actual, expected,
        "Makefile host exclusions drifted from Cargo manifests"
    );
}
