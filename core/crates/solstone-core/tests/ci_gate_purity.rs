// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "support/maturin_leaves.rs"]
mod maturin_leaves;

use maturin_leaves::{host_packaged_binaries, package_name};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary test directory");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
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

fn write_forbidden_shim(path: &Path) {
    let script = "#!/bin/sh\nset -eu\nprintf '%s %s\\n' \"$0\" \"$*\" >> \"$SOLSTONE_CI_SENTINEL\"\nexit 97\n";
    fs::write(path, script).expect("write forbidden interpreter shim");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make forbidden interpreter shim executable");
}

fn write_recording_cargo_shim(path: &Path) {
    let script = "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$SOLSTONE_CI_CARGO_LOG\"\ncase \"$*\" in\n  *'--bin solstone-core-depict'*)\n    printf '%s\\n' '{\"schema\":\"solstone-depict-error-v1\",\"reason\":\"malformed-request\"}' >&2\n    exit 1\n    ;;\n  *'--bin solstone-core-speakers-analyze'*)\n    printf '%s\\n' '{\"schema\":\"solstone-speaker-analyze-error-v1\",\"reason\":\"malformed-request\"}' >&2\n    exit 64\n    ;;\n  *'--bin solstone-core-vad-analyze'*)\n    printf '%s\\n' '{\"schema\":\"solstone-vad-error-v1\",\"reason\":\"malformed-request\"}' >&2\n    exit 64\n    ;;\nesac\n";
    fs::write(path, script).expect("write recording Cargo shim");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make recording Cargo shim executable");
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

/// Manually declared `[[test]]` targets and the features each one requires.
fn manual_test_targets(manifest: &str) -> Vec<(String, BTreeSet<String>)> {
    let mut targets = Vec::new();
    let mut current: Option<(Option<String>, BTreeSet<String>)> = None;

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if let Some((Some(name), features)) = current.take() {
                targets.push((name, features));
            }
            if trimmed == "[[test]]" {
                current = Some((None, BTreeSet::new()));
            }
            continue;
        }
        let Some((name, features)) = current.as_mut() else {
            continue;
        };
        if let Some(value) = trimmed.strip_prefix("name = ") {
            *name = Some(value.trim().trim_matches('"').to_owned());
        } else if let Some(value) = trimmed.strip_prefix("required-features = ") {
            features.extend(
                value
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|item| item.trim().trim_matches('"').to_owned())
                    .filter(|item| !item.is_empty()),
            );
        }
    }
    if let Some((Some(name), features)) = current {
        targets.push((name, features));
    }

    targets
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
            (!line.is_empty() && !line.starts_with('\t') && line.ends_with(':'))
                .then(|| rest.lines().take(index).map(|item| item.len() + 1).sum())
        })
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn make_ci_never_executes_forbidden_interpreters() {
    if env::var_os("SOLSTONE_CI_PURITY_REENTRY").is_some() {
        return;
    }

    let root = repo_root();
    let temp = TempDir::new("ci-gate-purity");
    let shim_dir = temp.path.join("shims");
    let venv_dir = temp.path.join("venv");
    let venv_bin = venv_dir.join("bin");
    let sentinel = temp.path.join("sentinel.log");
    let cargo_log = temp.path.join("cargo.log");
    let onnx_link_dir = temp.path.join("onnx-link");
    fs::create_dir(&shim_dir).expect("create shim directory");
    fs::create_dir_all(&venv_bin).expect("create poison virtualenv bin directory");
    fs::create_dir(&onnx_link_dir).expect("create fake ONNX link directory");
    fs::write(onnx_link_dir.join("libonnxruntime.so.1"), []).expect("write fake ONNX runtime");
    for name in ["python", "python3", "pytest", "ruff", "uv"] {
        write_forbidden_shim(&shim_dir.join(name));
        write_forbidden_shim(&venv_bin.join(name));
    }
    // The outer `make ci` invocation already performs every Cargo assertion.
    // Record the nested traversal instead of repeating the full workspace build.
    write_recording_cargo_shim(&shim_dir.join("cargo"));

    let path = format!(
        "{}:{}",
        shim_dir.display(),
        env::var("PATH").expect("PATH must be set for nested make ci")
    );
    let output = Command::new("make")
        .arg("ci")
        .arg(format!("VENV={}", venv_dir.display()))
        .arg(format!("VENV_BIN={}", venv_bin.display()))
        .arg(format!("PYTHON={}", venv_bin.join("python").display()))
        .arg(format!(
            "ONNX_RUNTIME_HOST_LINK_DIR={}",
            onnx_link_dir.display()
        ))
        .current_dir(root)
        .env("PATH", path)
        .env("SOLSTONE_CI_SENTINEL", &sentinel)
        .env("SOLSTONE_CI_CARGO_LOG", &cargo_log)
        .env("SOLSTONE_CI_PURITY_REENTRY", "1")
        .output()
        .expect("nested make ci should execute");

    assert!(
        output.status.success(),
        "nested make ci failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !sentinel.exists()
            || fs::read_to_string(&sentinel)
                .expect("read sentinel")
                .is_empty(),
        "make ci invoked a forbidden interpreter: {}",
        fs::read_to_string(&sentinel).unwrap_or_default(),
    );

    let cargo_invocations = fs::read_to_string(&cargo_log)
        .expect("nested make ci must traverse its Cargo-backed targets");
    let cargo_subcommands = cargo_invocations
        .lines()
        .filter_map(|invocation| invocation.split_whitespace().next())
        .collect::<Vec<_>>();
    let mut expected = vec!["fmt", "check", "clippy", "test", "build"];
    expected.extend(["run"; if cfg!(target_os = "linux") { 9 } else { 6 }]);
    if cfg!(target_os = "macos") {
        expected.push("check");
    }
    expected.push("check");
    expected.extend(["fetch", "deny"]);
    assert_eq!(
        cargo_subcommands, expected,
        "nested make ci did not traverse the complete Cargo command graph:\n{cargo_invocations}",
    );
}

#[test]
fn make_ci_builds_and_exercises_every_host_packaged_binary() {
    let root = repo_root();
    let makefile = makefile_text(&root);
    let expected = host_packaged_binaries(&root);
    let smoke = target_body(&makefile, "check-rust-shipped-binaries");
    let exercised = smoke
        .lines()
        .filter(|line| line.contains("cargo run"))
        .map(|line| {
            let words = line.split_whitespace().collect::<Vec<_>>();
            let package = words
                .windows(2)
                .find_map(|pair| (pair[0] == "-p").then_some(pair[1]))
                .expect("cargo run smoke must name its package");
            let binary = words
                .windows(2)
                .find_map(|pair| (pair[0] == "--bin").then_some(pair[1]))
                .expect("cargo run smoke must name its binary");
            (package.to_owned(), binary.to_owned())
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(
        exercised, expected,
        "the shipped-binary smoke gate must exactly match host-native maturin packaging leaves"
    );
    assert!(
        target_body(&makefile, "ci-under-poison").contains("$(MAKE) check-rust-shipped-binaries"),
        "make ci must retain the shipped-binary build and smoke gate"
    );
}

#[test]
fn make_ci_serializes_workspace_tests_that_compete_for_host_resources() {
    let makefile = makefile_text(&repo_root());
    let rust_test = target_body(&makefile, "check-rust-test");
    let cargo_test = rust_test
        .lines()
        .find(|line| line.trim_start().starts_with("cargo test "))
        .expect("check-rust-test must execute cargo test");
    let arguments = cargo_test.split_whitespace().collect::<Vec<_>>();

    assert!(
        arguments
            .windows(2)
            .any(|pair| pair[0] == "--" && pair[1] == "--test-threads=1"),
        "the full workspace suite must not make process and lock timeouts measure host contention"
    );
}

#[test]
fn make_ci_keeps_the_ios_gate_native_to_an_apple_sdk_host() {
    let makefile = makefile_text(&repo_root());
    let ci = target_body(&makefile, "ci-under-poison");
    let ios = target_body(&makefile, "check-rust-ios");
    let macos = target_body(&makefile, "check-rust-macos");

    assert!(
        ci.contains("$(MAKE) check-rust-ios"),
        "make ci must retain the iOS gate"
    );
    for protected in [
        "uname -s",
        "xcrun --sdk iphoneos --show-sdk-path",
        "--target $(IOS_TARGET)",
    ] {
        assert!(
            ios.contains(protected),
            "check-rust-ios lost its native-host assertion: {protected}"
        );
    }
    assert!(
        ci.contains("$(MAKE) check-rust-macos"),
        "make ci must retain the macOS cfg gate"
    );
    for protected in [
        "-p solstone-core-system",
        "-p solstone-core-local",
        "--target $(MACOS_TARGET)",
    ] {
        assert!(
            macos.contains(protected),
            "check-rust-macos lost its cfg-path assertion: {protected}"
        );
    }
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

/// A test kept out of `make ci` because it executes the Python implementation
/// must still be run by something. `check-differentials` is that something, and
/// this pins the two halves together: gating a test off the native gate without
/// naming it in the differential gate fails here rather than silently retiring
/// its coverage.
#[test]
fn every_differential_test_is_named_in_its_own_gate() {
    let root = repo_root();
    let core = root.join("core");
    let members = workspace_members(
        &fs::read_to_string(core.join("Cargo.toml")).expect("read workspace manifest"),
    );
    let makefile = fs::read_to_string(root.join("Makefile")).expect("read Makefile");

    let differentials = members
        .iter()
        .flat_map(|member| {
            let manifest = fs::read_to_string(core.join(member).join("Cargo.toml"))
                .expect("read member manifest");
            manual_test_targets(&manifest)
        })
        .filter(|(_name, features)| features.contains("differential"))
        .map(|(name, _features)| name)
        .collect::<BTreeSet<_>>();
    assert!(
        !differentials.is_empty(),
        "the differential feature must still gate at least one test target"
    );

    let gate = target_body(&makefile, "check-differentials");
    for name in &differentials {
        assert!(
            gate.contains(&format!("--test {name}")),
            "{name} is gated off make ci but not named in check-differentials"
        );
    }
    assert!(
        !target_body(&makefile, "check-rust-test").contains("differential"),
        "make ci must not enable the differential feature"
    );
}

/// Naming a test in the differential gate is not enough -- it has to be named
/// under the package that owns it.
///
/// `cargo test -p A --test t` where `t` lives in package B does not skip `t`;
/// it fails the whole invocation before running anything, so every other target
/// sharing that leg is skipped too. Measured: one such leg had been carrying
/// three targets and executing none of them, and the gate above was green the
/// entire time because it only ever checked that the name appeared somewhere in
/// the recipe.
#[test]
fn every_differential_leg_names_the_package_that_owns_its_tests() {
    let root = repo_root();
    let core = root.join("core");
    let members = workspace_members(
        &fs::read_to_string(core.join("Cargo.toml")).expect("read workspace manifest"),
    );

    // test-target name -> the packages that actually define it
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for member in &members {
        let directory = core.join(member);
        let manifest =
            fs::read_to_string(directory.join("Cargo.toml")).expect("read member manifest");
        let package = package_name(&manifest);
        for (name, _features) in manual_test_targets(&manifest) {
            owners.entry(name).or_default().insert(package.clone());
        }
        let Ok(entries) = fs::read_dir(directory.join("tests")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                let name = path
                    .file_stem()
                    .expect("test file stem")
                    .to_string_lossy()
                    .into_owned();
                owners.entry(name).or_default().insert(package.clone());
            }
        }
    }

    let makefile = makefile_text(&root);
    let gate = target_body(&makefile, "check-differentials");
    let mut checked = 0_usize;
    for leg in gate
        .split('"')
        .filter(|leg| leg.trim_start().starts_with("-p "))
    {
        let words = leg.split_whitespace().collect::<Vec<_>>();
        let package = words[words.iter().position(|word| *word == "-p").expect("-p") + 1];
        for (index, word) in words.iter().enumerate() {
            if *word != "--test" {
                continue;
            }
            let target = words[index + 1];
            let owning = owners
                .get(target)
                .unwrap_or_else(|| panic!("no package in the workspace defines a test {target}"));
            assert!(
                owning.contains(package),
                "check-differentials runs `-p {package} --test {target}`, but {target} \
                 lives in {owning:?}. That leg fails before running anything, so every \
                 target sharing it is skipped"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "the differential gate named no targets -- the leg parsing is wrong"
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

    let expected_sanity = BTreeSet::from([
        "solstone-core-speakers-analyze".to_owned(),
        "solstone-core-speakers-onnx".to_owned(),
        "solstone-core-vad-analyze".to_owned(),
    ]);
    assert_eq!(
        expected, expected_sanity,
        "unexpected ONNX host exclusion closure"
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

    for target in [
        "build",
        "check-rust-msrv",
        "check-rust-clippy",
        "check-rust-test",
    ] {
        assert!(
            target_body(&makefile, target).contains("$(RUST_HOST_EXCLUDES)"),
            "{target} must use RUST_HOST_EXCLUDES"
        );
    }
}
