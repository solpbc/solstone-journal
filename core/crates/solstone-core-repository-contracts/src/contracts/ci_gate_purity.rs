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

#[path = "../../../solstone-core/tests/support/maturin_leaves.rs"]
mod maturin_leaves;

use maturin_leaves::{host_packaged_binaries, package_name};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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

fn write_executable(path: &Path, script: &str) {
    fs::write(path, script).expect("write executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

fn write_native_cargo_shim(path: &Path) {
    if cfg!(target_os = "macos") {
        let source = path.with_extension("c");
        fs::write(
            &source,
            r#"#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(int argc, char **argv) {
    FILE *args = fopen(getenv("SOLSTONE_CARGO_ARGV"), "wb");
    for (int i = 1; i < argc; i++) fwrite(argv[i], 1, strlen(argv[i]) + 1, args);
    fclose(args);
    FILE *env = fopen(getenv("SOLSTONE_CARGO_ENV"), "w");
    const char *names[] = {"ORT_PREFER_DYNAMIC_LINK", "ORT_LIB_PATH", "DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"};
    for (int i = 0; i < 4; i++) {
        const char *value = getenv(names[i]);
        fprintf(env, "%s=%s\n", names[i], value ? value : "");
    }
    fclose(env);
    return 0;
}
"#,
        )
        .expect("write native Cargo recorder source");
        let output = Command::new("cc")
            .arg(&source)
            .arg("-o")
            .arg(path)
            .output()
            .expect("compile native Cargo recorder");
        assert!(
            output.status.success(),
            "compile native Cargo recorder: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    write_executable(
        path,
        "#!/bin/sh\nset -eu\nprintf '%s\\0' \"$@\" > \"$SOLSTONE_CARGO_ARGV\"\nprintf 'ORT_PREFER_DYNAMIC_LINK=%s\\nORT_LIB_PATH=%s\\nDYLD_LIBRARY_PATH=%s\\nLD_LIBRARY_PATH=%s\\n' \"${ORT_PREFER_DYNAMIC_LINK-}\" \"${ORT_LIB_PATH-}\" \"${DYLD_LIBRARY_PATH-}\" \"${LD_LIBRARY_PATH-}\" > \"$SOLSTONE_CARGO_ENV\"\n",
    );
}

fn write_rustup_shim(path: &Path, body: &str) {
    write_executable(path, &format!("#!/bin/sh\nset -eu\n{body}\n"));
}

fn replace_once(text: &mut String, old: &str, new: &str) {
    let count = text.matches(old).count();
    assert_eq!(count, 1, "fixture seam must occur exactly once: {old}");
    *text = text.replacen(old, new, 1);
}

fn write_host_makefile(root: &Path, system: &str, arch: &str) {
    let mut makefile = makefile_text(&repo_root());
    replace_once(
        &mut makefile,
        "override HOST_SYSTEM := $(shell /usr/bin/uname -s)",
        &format!("override HOST_SYSTEM := {system}"),
    );
    replace_once(
        &mut makefile,
        "override HOST_ARCH := $(shell /usr/bin/uname -m)",
        &format!("override HOST_ARCH := {arch}"),
    );
    let digest_key = match (system, arch) {
        ("Linux", "x86_64" | "amd64") => "LINUX_X86_64",
        ("Linux", "aarch64" | "arm64") => "LINUX_AARCH64",
        ("Darwin", "arm64" | "aarch64") => "MACOS_ARM64",
        _ => "",
    };
    if !digest_key.is_empty() {
        replace_once(
            &mut makefile,
            &format!("override ONNX_RUNTIME_{digest_key}_DIGEST := "),
            &format!("override ONNX_RUNTIME_{digest_key}_DIGEST := {EMPTY_SHA256}#"),
        );
    }
    // Fixtures must use a real checksum implementation available on the
    // executing host, independently of the host tuple they simulate.
    if system == "Linux" && !Path::new("/usr/bin/sha256sum").is_file() {
        replace_once(
            &mut makefile,
            "override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/sha256sum",
            "override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/shasum",
        );
        replace_once(
            &mut makefile,
            "override ONNX_RUNTIME_HOST_HASH_ARGS :=\n",
            "override ONNX_RUNTIME_HOST_HASH_ARGS := -a 256\n",
        );
    } else if system == "Darwin" && !Path::new("/usr/bin/shasum").is_file() {
        replace_once(
            &mut makefile,
            "override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/shasum",
            "override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/sha256sum",
        );
        replace_once(
            &mut makefile,
            "override ONNX_RUNTIME_HOST_HASH_ARGS := -a 256",
            "override ONNX_RUNTIME_HOST_HASH_ARGS :=",
        );
    }
    fs::write(root.join("Makefile"), makefile).expect("write host Makefile fixture");
    fs::create_dir_all(root.join("core")).expect("create fixture core directory");
    fs::write(root.join("core/Cargo.toml"), "[workspace]\nmembers = []\n")
        .expect("write fixture Cargo manifest");
}

fn replace_makefile_once(root: &Path, old: &str, new: &str) {
    let path = root.join("Makefile");
    let mut makefile = fs::read_to_string(&path).expect("read fixture Makefile");
    replace_once(&mut makefile, old, new);
    fs::write(path, makefile).expect("rewrite fixture Makefile");
}

fn replace_linux_fixture_verifier(root: &Path, verifier: &Path) {
    let (fixture_verifier, replacement_verifier) = if Path::new("/usr/bin/sha256sum").is_file() {
        (
            "override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/sha256sum".to_owned(),
            format!(
                "override ONNX_RUNTIME_HOST_HASH_PROGRAM := {}",
                verifier.display()
            ),
        )
    } else {
        (
                "override ONNX_RUNTIME_HOST_HASH_PROGRAM := /usr/bin/shasum\noverride ONNX_RUNTIME_HOST_LOADER_ENV := LD_LIBRARY_PATH".to_owned(),
                format!(
                    "override ONNX_RUNTIME_HOST_HASH_PROGRAM := {}\noverride ONNX_RUNTIME_HOST_LOADER_ENV := LD_LIBRARY_PATH",
                    verifier.display()
                ),
            )
    };
    replace_makefile_once(root, &fixture_verifier, &replacement_verifier);
}

fn seed_runtime(root: &Path, target: &str, names: &[&str], bytes: &[u8]) -> PathBuf {
    let link_dir = root
        .join("target/speakers-analyze-runtime-link")
        .join(target);
    fs::create_dir_all(&link_dir).expect("create runtime fixture directory");
    for name in names {
        fs::write(link_dir.join(name), bytes).expect("write runtime fixture");
    }
    link_dir
}

fn fixture_path(shims: &Path) -> String {
    format!(
        "{}:{}",
        shims.display(),
        env::var("PATH").expect("PATH must be set")
    )
}

fn nul_argv(path: &Path) -> Vec<String> {
    fs::read(path)
        .expect("read NUL argv log")
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8(part.to_vec()).expect("argv must be UTF-8"))
        .collect()
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

fn assert_gate_never_executes_forbidden_interpreters(gate: &str, expected: &[&str]) {
    if env::var_os("SOLSTONE_CI_PURITY_REENTRY").is_some() {
        return;
    }

    let temp = TempDir::new(&format!("{gate}-gate-purity"));
    let root = &temp.path;
    let system = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    let arch_output = Command::new("/usr/bin/uname")
        .arg("-m")
        .output()
        .expect("inspect fixture host architecture");
    assert!(arch_output.status.success());
    let arch = String::from_utf8(arch_output.stdout)
        .expect("host architecture is UTF-8")
        .trim()
        .to_owned();
    write_host_makefile(root, system, &arch);
    let shim_dir = root.join("shims");
    let venv_dir = root.join("venv");
    let venv_bin = venv_dir.join("bin");
    let sentinel = temp.path.join("sentinel.log");
    let cargo_log = temp.path.join("cargo.log");
    fs::create_dir(&shim_dir).expect("create shim directory");
    fs::create_dir_all(&venv_bin).expect("create poison virtualenv bin directory");
    let onnx_names = if cfg!(target_os = "macos") {
        ["libonnxruntime.1.25.0.dylib", "libonnxruntime.dylib"].as_slice()
    } else {
        [
            "libonnxruntime.so.1.25.0",
            "libonnxruntime.so.1",
            "libonnxruntime.so",
        ]
        .as_slice()
    };
    seed_runtime(
        root,
        if cfg!(target_os = "macos") {
            "macos-arm64"
        } else if matches!(arch.as_str(), "aarch64" | "arm64") {
            "linux-aarch64"
        } else {
            "linux-x86_64"
        },
        onnx_names,
        &[],
    );
    let (pdf_target, pdf_filename) = if cfg!(target_os = "macos") {
        ("macos-arm64", "libpdfium.dylib")
    } else if matches!(arch.as_str(), "aarch64" | "arm64") {
        ("linux-aarch64", "libpdfium.so")
    } else {
        ("linux-x86_64", "libpdfium.so")
    };
    let pdf_link_dir = root.join("target/pdfium-runtime-link").join(pdf_target);
    fs::create_dir_all(&pdf_link_dir).expect("create fake PDFium link directory");
    fs::write(pdf_link_dir.join(pdf_filename), []).expect("write fake PDFium runtime");
    for name in ["python", "python3", "pytest", "ruff", "uv"] {
        write_forbidden_shim(&shim_dir.join(name));
        write_forbidden_shim(&venv_bin.join(name));
    }
    // The outer gate invocation already performs every Cargo assertion.
    // Record the nested traversal instead of repeating the full workspace build.
    write_recording_cargo_shim(&shim_dir.join("cargo"));

    let path = format!(
        "{}:{}",
        shim_dir.display(),
        env::var("PATH").expect("PATH must be set for nested Rust gate")
    );
    let output = Command::new("make")
        .arg(gate)
        .arg(format!("VENV={}", venv_dir.display()))
        .arg(format!("VENV_BIN={}", venv_bin.display()))
        .arg(format!("PYTHON={}", venv_bin.join("python").display()))
        .current_dir(root)
        .env("PATH", path)
        .env("SOLSTONE_CI_SENTINEL", &sentinel)
        .env("SOLSTONE_CI_CARGO_LOG", &cargo_log)
        .env("SOLSTONE_CI_PURITY_REENTRY", "1")
        .output()
        .expect("nested Rust gate should execute");

    assert!(
        output.status.success(),
        "nested make {gate} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !sentinel.exists()
            || fs::read_to_string(&sentinel)
                .expect("read sentinel")
                .is_empty(),
        "make {gate} invoked a forbidden interpreter: {}",
        fs::read_to_string(&sentinel).unwrap_or_default(),
    );

    let cargo_invocations = fs::read_to_string(&cargo_log)
        .expect("nested Rust gate must traverse its Cargo-backed targets");
    let cargo_subcommands = cargo_invocations
        .lines()
        .filter_map(|invocation| invocation.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(
        cargo_subcommands, expected,
        "nested make {gate} traversed the wrong Cargo command graph:\n{cargo_invocations}",
    );
}

#[test]
fn make_ci_never_executes_forbidden_interpreters() {
    assert_gate_never_executes_forbidden_interpreters("ci", &["fmt", "clippy", "test"]);
}

#[test]
fn make_ci_full_never_executes_forbidden_interpreters() {
    // Dependency policy runs before the long workspace suite so a known test
    // failure cannot hide it. The trailing "test" is
    // check-rust-describe-cli-stubs, immediately after that workspace leg.
    // ONNX and PDF each add one Linux-only test.
    let mut expected = vec!["fmt", "check", "clippy", "fetch", "deny", "test", "test"];
    if cfg!(target_os = "linux") {
        expected.extend(["test", "test"]);
    }
    expected.push("build");
    expected.extend(["run"; if cfg!(target_os = "linux") { 10 } else { 7 }]);
    if cfg!(target_os = "macos") {
        expected.extend(["check", "test"]);
    }
    assert_gate_never_executes_forbidden_interpreters("ci-full", &expected);
}

#[test]
fn make_ci_full_checks_dependency_policy_before_the_long_workspace_suite() {
    let makefile = makefile_text(&repo_root());
    let ci = target_body(&makefile, "ci-full-under-poison");
    let deny = ci
        .find("$(MAKE) check-rust-deny")
        .expect("make ci-full must retain the dependency-policy gate");
    let workspace_tests = ci
        .find("$(MAKE) check-rust-test")
        .expect("make ci-full must retain the full workspace test gate");

    assert!(
        deny < workspace_tests,
        "dependency policy must run before the long workspace suite so a known test failure cannot hide it"
    );
}

#[test]
fn make_ci_full_builds_and_exercises_every_host_packaged_binary() {
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
        target_body(&makefile, "ci-full-under-poison")
            .contains("$(MAKE) check-rust-shipped-binaries"),
        "make ci-full must retain the shipped-binary build and smoke gate"
    );
    assert!(
        !target_body(&makefile, "ci-under-poison").contains("$(MAKE) check-rust-shipped-binaries"),
        "make ci must not link or run the shipped-binary smoke gate"
    );
}

/// Every crate `RUST_HOST_EXCLUDES` removes from the workspace test selection
/// must be named in a package list the full gate actually runs.
///
/// This is the guard the tree did not have. The two lists were written months
/// apart and nothing tied them together, so three crates were excluded from
/// `check-rust-test` while only one of them was picked up by a target of its
/// own -- 33 `#[test]`s that no `make` target ran, indistinguishable from
/// coverage. Adding a fourth `--exclude` without adding it to
/// `ONNX_HOST_TEST_PACKAGES` reds here.
///
/// Pairs with `rust_host_excludes_match_the_workspace_onnx_closure`, which
/// answers *why* a crate is excluded (it is in the ONNX dependency closure).
/// Together they close the loop: a new `ort` consumer must join
/// `RUST_HOST_EXCLUDES` or that test reds, and once it does it must join
/// `ONNX_HOST_TEST_PACKAGES` or this one does.
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
        .find(|line| line.starts_with("ONNX_HOST_TEST_PACKAGES :="))
        .expect("ONNX_HOST_TEST_PACKAGES must be defined");
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
    assert!(
        target_body(&makefile, "ci-full-under-poison").contains("$(MAKE) check-rust-onnx-test"),
        "make ci-full must run the tests of the crates it excludes from the workspace selection"
    );
}

/// The race gate may name only supervisor tests that use W4b's explicit
/// inconclusive outcome. Removing one from the Makefile list must therefore
/// red this source-derived guard rather than silently reducing coverage.
#[test]
fn every_w4b_supervisor_test_is_named_in_rust_race_gate() {
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
        "RUST_RACE_TEST_TARGETS must exactly name W4b-converted supervisor tests"
    );
    assert!(
        target_body(&makefile, "check-rust-race").contains("$(RUST_RACE_TEST_TARGETS)"),
        "check-rust-race must reference RUST_RACE_TEST_TARGETS, not a hand copy"
    );
}

#[test]
fn make_ci_full_serializes_workspace_tests_that_compete_for_host_resources() {
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
fn make_ci_runs_only_library_and_binary_unit_harnesses() {
    let makefile = makefile_text(&repo_root());
    let ci = target_body(&makefile, "ci-under-poison");
    assert!(ci.contains("$(MAKE) check-rust-fmt"));
    assert!(ci.contains("$(MAKE) check-rust-clippy"));
    assert!(ci.contains("$(MAKE) check-rust-unit"));
    for forbidden in [
        "check-rust-msrv",
        "check-rust-test",
        "check-rust-describe-cli-stubs",
        "check-rust-onnx-test",
        "check-rust-pdf-test",
        "check-rust-shipped-binaries",
        "check-rust-ios",
        "check-rust-macos",
        "check-rust-deny",
    ] {
        assert!(
            !ci.contains(&format!("$(MAKE) {forbidden}")),
            "make ci must not traverse the full-gate leg {forbidden}"
        );
    }

    let unit = target_body(&makefile, "check-rust-unit");
    let cargo_test = unit
        .lines()
        .find(|line| line.trim_start().starts_with("cargo test "))
        .expect("check-rust-unit must execute cargo test");
    for required in [
        "--manifest-path $(RUST_MANIFEST)",
        "--workspace",
        "$(RUST_HOST_EXCLUDES)",
        "--lib",
        "--bins",
        "--locked",
        "-- --test-threads=1",
    ] {
        assert!(
            cargo_test.contains(required),
            "check-rust-unit lost required selection: {required}"
        );
    }
    for forbidden in [
        "--tests",
        " --test ",
        "--examples",
        "--benches",
        "--doc",
        "--all-targets",
    ] {
        assert!(
            !cargo_test.contains(forbidden),
            "check-rust-unit widened beyond unit harnesses with {forbidden}"
        );
    }
}

#[test]
fn efficient_ci_keeps_all_target_static_compilation() {
    let makefile = makefile_text(&repo_root());
    let clippy = target_body(&makefile, "check-rust-clippy");
    let invocation = clippy
        .lines()
        .find(|line| line.trim_start().starts_with("cargo clippy "))
        .expect("check-rust-clippy must execute cargo clippy");
    for required in [
        "--manifest-path $(RUST_MANIFEST)",
        "--workspace",
        "$(RUST_HOST_EXCLUDES)",
        "--all-targets",
        "--locked",
        "-- -D warnings",
    ] {
        assert!(
            invocation.contains(required),
            "efficient CI lost static-compilation coverage: {required}"
        );
    }
}

#[test]
fn public_rust_gates_share_poison_and_refuse_internal_entrypoints() {
    let makefile = makefile_text(&repo_root());
    for (public, internal) in [
        ("ci", "ci-under-poison"),
        ("ci-full", "ci-full-under-poison"),
    ] {
        let outer = target_body(&makefile, public);
        assert!(
            outer.contains("$(call run-rust-gate-under-poison"),
            "make {public} bypasses the shared interpreter-poison wrapper"
        );
        assert!(outer.contains(internal));

        let inner = target_body(&makefile, internal);
        assert!(inner.contains("SOLSTONE_CI_POISONED"));
        assert!(inner.contains(&format!("run 'make {public}'")));
    }
}

#[test]
fn make_ci_full_names_the_manual_rust_race_gate() {
    let makefile = makefile_text(&repo_root());
    let ci = target_body(&makefile, "ci-full-under-poison");
    assert!(
        ci.contains("check-rust-race"),
        "make ci-full closing output must name the manual check-rust-race gate"
    );
    assert!(
        !ci.contains("$(MAKE) check-rust-race"),
        "check-rust-race must remain manually invoked, outside make ci-full"
    );
}

#[test]
fn manual_race_gate_is_selectable_without_uv() {
    let root = repo_root();
    let dry_run = Command::new("make")
        .args(["-n", "check-rust-race"])
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&root)
        .output()
        .expect("uv-free make dry run starts");
    assert!(
        dry_run.status.success(),
        "check-rust-race must be selectable without uv: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
}

#[test]
fn make_ci_full_keeps_apple_gates_native_to_apple_sdk_hosts() {
    let makefile = makefile_text(&repo_root());
    let ci = target_body(&makefile, "ci-full-under-poison");
    let ios = target_body(&makefile, "check-rust-ios");
    let macos = target_body(&makefile, "check-rust-macos");

    assert!(
        ci.contains("$(MAKE) check-rust-ios"),
        "make ci-full must retain the iOS gate"
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
        "make ci-full must retain the macOS cfg gate"
    );
    for protected in [
        "cargo test",
        "--manifest-path core/Cargo.toml",
        "--workspace",
        "--all-targets",
        "--no-run",
        "--target aarch64-apple-darwin",
        "--locked",
    ] {
        assert!(
            macos.contains(protected),
            "check-rust-macos lost its cfg-path assertion: {protected}"
        );
    }
    for forbidden in [" -p ", "--exclude", "--lib", "cargo check"] {
        assert!(
            !macos.contains(forbidden),
            "check-rust-macos silently narrowed the workspace with {forbidden}"
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

fn python_runtime_specs(text: &str) -> BTreeMap<String, RuntimeSpecContract> {
    let target_start = text
        .find("TARGETS = {")
        .expect("stage script must define TARGETS");
    let mut specs = BTreeMap::new();
    let mut current_key: Option<String> = None;
    let mut current_digest: Option<String> = None;
    let mut current_links = Vec::new();
    let mut reading_links = false;
    let mut reading_digest = false;

    for line in text[target_start..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed == "}" {
            break;
        }
        if trimmed.starts_with('"') && trimmed.contains(": TargetSpec(") {
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
            reading_digest = false;
            continue;
        }
        if trimmed.starts_with("runtime_sha256") {
            let value = trimmed
                .split_once('=')
                .map(|(_name, value)| value)
                .expect("runtime_sha256 assignment");
            current_digest = quoted_values(value).into_iter().next();
            reading_digest = current_digest.is_none();
            continue;
        }
        if reading_digest {
            current_digest = quoted_values(trimmed).into_iter().next();
            if current_digest.is_some() {
                reading_digest = false;
            }
            continue;
        }
        if trimmed.starts_with("link_names=") {
            reading_links = true;
        }
        if reading_links {
            current_links.extend(quoted_values(trimmed));
            if trimmed.ends_with("),") {
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
    let script = fs::read_to_string(root.join("scripts/stage_speakers_analyze_runtime.py"))
        .expect("read runtime staging script");
    let actual = python_runtime_specs(&script);
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
        let expected = actual.get(script_key).expect("script target must exist");
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
            "Make link names drifted from scripts/stage_speakers_analyze_runtime.py for {script_key}"
        );
    }
}

#[test]
fn runtime_target_parser_ignores_decoys_and_formatting() {
    let fixture = r#"
DECOY = TargetSpec(
    runtime_sha256="wrong",
    link_names=("wrong",),
)
TARGETS = {
  "fixture": TargetSpec(
      runtime_sha256 =
          "abc",
      link_names=(
          "one",
          "two",
      ),
  ),
}
"#;
    let parsed = python_runtime_specs(fixture);
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
fn native_macos_gate_records_the_full_workspace_test_compile() {
    for arch in ["arm64", "aarch64"] {
        for prior_dyld in [None, Some("prior-dyld")] {
            let temp = TempDir::new("native-macos-gate");
            write_host_makefile(&temp.path, "Darwin", arch);
            let shims = temp.path.join("shims");
            fs::create_dir(&shims).expect("create shims");
            write_native_cargo_shim(&shims.join("cargo"));
            write_rustup_shim(&shims.join("rustup"), "printf '%s\\n' aarch64-apple-darwin");
            let argv_log = temp.path.join("cargo.argv");
            let env_log = temp.path.join("cargo.env");
            let link_dir = seed_runtime(
                &temp.path,
                "macos-arm64",
                &["libonnxruntime.1.25.0.dylib", "libonnxruntime.dylib"],
                &[],
            );
            let mut command = Command::new("make");
            command
                .arg("check-rust-macos")
                .current_dir(&temp.path)
                .env("PATH", fixture_path(&shims))
                .env("SOLSTONE_CARGO_ARGV", &argv_log)
                .env("SOLSTONE_CARGO_ENV", &env_log);
            if let Some(value) = prior_dyld {
                // macOS strips inherited DYLD_* before /usr/bin/make starts;
                // the supported preservation seam is an explicit Make value.
                command.arg(format!("DYLD_LIBRARY_PATH={value}"));
            } else {
                command.env_remove("DYLD_LIBRARY_PATH");
            }
            let output = command.output().expect("run simulated native macOS gate");
            assert!(
                output.status.success(),
                "simulated macOS gate failed for {arch}:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                nul_argv(&argv_log),
                [
                    "test",
                    "--manifest-path",
                    "core/Cargo.toml",
                    "--workspace",
                    "--all-targets",
                    "--no-run",
                    "--target",
                    "aarch64-apple-darwin",
                    "--locked",
                ]
            );
            let recorded_env = fs::read_to_string(&env_log).expect("read Cargo env log");
            assert!(recorded_env.contains("ORT_PREFER_DYNAMIC_LINK=true"));
            let canonical_link_dir = fs::canonicalize(&link_dir).expect("canonical runtime dir");
            assert!(
                recorded_env.contains(&format!("ORT_LIB_PATH={}", canonical_link_dir.display()))
            );
            let expected_dyld = prior_dyld.map_or_else(
                || canonical_link_dir.display().to_string(),
                |prior| format!("{}:{prior}", canonical_link_dir.display()),
            );
            assert!(
                recorded_env.contains(&format!("DYLD_LIBRARY_PATH={expected_dyld}")),
                "DYLD loader path was not composed correctly:\n{recorded_env}"
            );
        }
    }
}

#[test]
fn native_macos_gate_ignores_make_control_plane_overrides() {
    let temp = TempDir::new("native-macos-immutable");
    write_host_makefile(&temp.path, "Darwin", "arm64");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create shims");
    write_native_cargo_shim(&shims.join("cargo"));
    write_rustup_shim(&shims.join("rustup"), "printf '%s\n' aarch64-apple-darwin");
    let argv_log = temp.path.join("cargo.argv");
    let env_log = temp.path.join("cargo.env");
    let link_dir = seed_runtime(
        &temp.path,
        "macos-arm64",
        &["libonnxruntime.1.25.0.dylib", "libonnxruntime.dylib"],
        &[],
    );
    let output = Command::new("make")
        .arg("check-rust-macos")
        .args([
            "SHELL=/bin/false",
            ".SHELLFLAGS=-n",
            "HOST_SYSTEM=Linux",
            "HOST_ARCH=x86_64",
            "CURDIR=/tmp/solstone-redirected",
            "REPO_ROOT=/tmp/solstone-redirected",
            "ONNX_RUNTIME_HOST_TARGET=linux-x86_64",
            "ONNX_RUNTIME_HOST_DIGEST=attacker-chosen",
            "ONNX_RUNTIME_HOST_LINK_DIR=/tmp/solstone-redirected",
        ])
        .current_dir(&temp.path)
        .env("PATH", fixture_path(&shims))
        .env("SOLSTONE_CARGO_ARGV", &argv_log)
        .env("SOLSTONE_CARGO_ENV", &env_log)
        .output()
        .expect("run immutable macOS gate fixture");
    assert!(
        output.status.success(),
        "Make control-plane override changed the gate:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        nul_argv(&argv_log).first().map(String::as_str),
        Some("test")
    );
    let recorded_env = fs::read_to_string(env_log).expect("read immutable gate environment");
    assert!(recorded_env.contains(&format!(
        "ORT_LIB_PATH={}",
        fs::canonicalize(link_dir)
            .expect("canonical runtime path")
            .display()
    )));
    assert!(!recorded_env.contains("solstone-redirected"));
}

#[test]
fn pdf_runtime_mapping_ignores_make_control_plane_overrides() {
    for (system, arch, expected_target, expected_library) in [
        ("Linux", "x86_64", "linux-x86_64", "libpdfium.so"),
        ("Linux", "aarch64", "linux-aarch64", "libpdfium.so"),
        ("Darwin", "arm64", "macos-arm64", "libpdfium.dylib"),
    ] {
        let temp = TempDir::new("pdf-runtime-immutable");
        write_host_makefile(&temp.path, system, arch);
        let makefile_path = temp.path.join("Makefile");
        let mut makefile = fs::read_to_string(&makefile_path).expect("read fixture Makefile");
        makefile.push_str(
            "\nprint-pdf-runtime-contract:\n\t@printf '%s|%s|%s\\n' \"$(PDF_RUNTIME_HOST_TARGET)\" \"$(PDF_RUNTIME_HOST_LIBRARY)\" \"$(PDF_RUNTIME_HOST_LINK_DIR)\"\n",
        );
        fs::write(&makefile_path, makefile).expect("extend fixture Makefile");

        let output = Command::new("make")
            .args(["--no-print-directory", "print-pdf-runtime-contract"])
            .args([
                "HOST_SYSTEM=Plan9",
                "HOST_ARCH=mips",
                "REPO_ROOT=/tmp/solstone-redirected",
                "PDF_RUNTIME_HOST_TARGET=forced",
                "PDF_RUNTIME_HOST_LIBRARY=forced.bin",
                "PDF_RUNTIME_HOST_LINK_DIR=/tmp/solstone-redirected",
            ])
            .current_dir(&temp.path)
            .output()
            .expect("print immutable PDF runtime mapping");
        assert!(output.status.success());
        let expected_dir = fs::canonicalize(&temp.path)
            .expect("canonical fixture root")
            .join("target/pdfium-runtime-link")
            .join(expected_target);
        assert_eq!(
            String::from_utf8(output.stdout)
                .expect("mapping output is UTF-8")
                .trim(),
            format!(
                "{expected_target}|{expected_library}|{}",
                expected_dir.display()
            )
        );
    }
}

#[test]
fn unsupported_pdf_host_fails_before_staging() {
    let temp = TempDir::new("unsupported-pdf-host");
    write_host_makefile(&temp.path, "Plan9", "mips");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create shims");
    write_forbidden_shim(&shims.join("python3"));
    let sentinel = temp.path.join("python-sentinel");

    let output = Command::new("make")
        .arg("check-rust-pdf-stage")
        .current_dir(&temp.path)
        .env("PATH", fixture_path(&shims))
        .env("SOLSTONE_CI_SENTINEL", &sentinel)
        .output()
        .expect("run unsupported PDF host gate");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Plan9/mips"));
    assert!(
        !sentinel.exists(),
        "unsupported PDF host invoked the staging interpreter"
    );
}

#[test]
fn macos_gate_rejects_each_missing_or_corrupt_runtime_before_cargo() {
    let names = ["libonnxruntime.1.25.0.dylib", "libonnxruntime.dylib"];
    for victim in names {
        for state in ["missing", "corrupt"] {
            let temp = TempDir::new("macos-runtime-negative");
            write_host_makefile(&temp.path, "Darwin", "arm64");
            let shims = temp.path.join("shims");
            fs::create_dir(&shims).expect("create shims");
            write_native_cargo_shim(&shims.join("cargo"));
            write_rustup_shim(&shims.join("rustup"), "printf '%s\\n' aarch64-apple-darwin");
            let link_dir = seed_runtime(&temp.path, "macos-arm64", &names, &[]);
            if state == "missing" {
                fs::remove_file(link_dir.join(victim)).expect("remove runtime fixture");
            } else {
                fs::write(link_dir.join(victim), b"corrupt").expect("corrupt runtime fixture");
            }
            let argv_log = temp.path.join("cargo.argv");
            let output = Command::new("make")
                .arg("check-rust-macos")
                .current_dir(&temp.path)
                .env("PATH", fixture_path(&shims))
                .env("SOLSTONE_CARGO_ARGV", &argv_log)
                .env("SOLSTONE_CARGO_ENV", temp.path.join("cargo.env"))
                .output()
                .expect("run macOS runtime negative");
            assert!(!output.status.success(), "{victim} {state} must fail");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains(victim),
                "diagnostic omitted {victim}: {stderr}"
            );
            assert!(stderr.contains("make check-rust-onnx-stage"));
            assert!(!argv_log.exists(), "Cargo ran with {victim} {state}");
        }
    }
}

#[test]
fn macos_gate_distinguishes_rustup_failure_from_a_missing_target() {
    for (rustup_body, expected) in [
        ("exit 23", "rustup failed to inspect installed targets"),
        (
            "printf '%s\\n' x86_64-unknown-linux-gnu",
            "Rust target aarch64-apple-darwin is required",
        ),
    ] {
        let temp = TempDir::new("macos-rustup-negative");
        write_host_makefile(&temp.path, "Darwin", "arm64");
        let shims = temp.path.join("shims");
        fs::create_dir(&shims).expect("create shims");
        write_native_cargo_shim(&shims.join("cargo"));
        write_rustup_shim(&shims.join("rustup"), rustup_body);
        let argv_log = temp.path.join("cargo.argv");
        let output = Command::new("make")
            .arg("check-rust-macos")
            .current_dir(&temp.path)
            .env("PATH", fixture_path(&shims))
            .env("SOLSTONE_CARGO_ARGV", &argv_log)
            .env("SOLSTONE_CARGO_ENV", temp.path.join("cargo.env"))
            .output()
            .expect("run rustup negative");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert!(
            !argv_log.exists(),
            "Cargo ran after rustup prerequisite failure"
        );
    }
}

#[test]
fn onnx_stage_is_a_noop_when_every_pinned_link_is_healthy() {
    let temp = TempDir::new("onnx-stage-healthy");
    write_host_makefile(&temp.path, "Linux", "x86_64");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create shims");
    let python_sentinel = temp.path.join("python-ran");
    write_executable(
        &shims.join("python3"),
        "#!/bin/sh\n: > \"$SOLSTONE_PYTHON_SENTINEL\"\nexit 97\n",
    );
    seed_runtime(
        &temp.path,
        "linux-x86_64",
        &[
            "libonnxruntime.so.1.25.0",
            "libonnxruntime.so.1",
            "libonnxruntime.so",
        ],
        &[],
    );
    let output = Command::new("make")
        .arg("check-rust-onnx-stage")
        .current_dir(&temp.path)
        .env("PATH", fixture_path(&shims))
        .env("SOLSTONE_PYTHON_SENTINEL", &python_sentinel)
        .output()
        .expect("run healthy stage validator");
    assert!(
        output.status.success(),
        "healthy stage validation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!python_sentinel.exists(), "healthy staging invoked Python");
}

#[test]
fn onnx_stage_repairs_then_independently_validates_every_link() {
    for (victim, post_state) in [
        ("libonnxruntime.so.1.25.0", "missing"),
        ("libonnxruntime.so.1", "corrupt"),
        ("libonnxruntime.so", "missing"),
    ] {
        let temp = TempDir::new("onnx-stage-postcondition");
        write_host_makefile(&temp.path, "Linux", "x86_64");
        let shims = temp.path.join("shims");
        fs::create_dir(&shims).expect("create shims");
        let python_log = temp.path.join("python.log");
        write_executable(
            &shims.join("python3"),
            &format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\0' \"$@\" > \"$SOLSTONE_PYTHON_LOG\"\ndir=target/speakers-analyze-runtime-link/linux-x86_64\nmkdir -p \"$dir\"\n: > \"$dir/libonnxruntime.so.1.25.0\"\n: > \"$dir/libonnxruntime.so.1\"\n: > \"$dir/libonnxruntime.so\"\n{}\n",
                if post_state == "missing" {
                    format!("rm -f \"$dir/{victim}\"")
                } else {
                    format!("printf corrupt > \"$dir/{victim}\"")
                }
            ),
        );
        let output = Command::new("make")
            .arg("check-rust-onnx-stage")
            .current_dir(&temp.path)
            .env("PATH", fixture_path(&shims))
            .env("SOLSTONE_PYTHON_LOG", &python_log)
            .output()
            .expect("run staging postcondition negative");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(victim),
            "postvalidation omitted {victim}: {stderr}"
        );
        assert!(stderr.contains("make check-rust-onnx-stage"));
        let python_argv = nul_argv(&python_log);
        assert_eq!(
            python_argv.iter().filter(|arg| *arg == "--target").count(),
            1,
            "repair must invoke the staging script exactly once"
        );
        assert!(
            python_argv
                .windows(2)
                .any(|pair| pair == ["--target", "linux-x86_64"])
        );
    }
}

#[test]
fn onnx_stage_preserves_a_failed_staging_status_even_if_files_were_repaired() {
    let temp = TempDir::new("onnx-stage-failed-repair");
    write_host_makefile(&temp.path, "Linux", "x86_64");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create shims");
    write_executable(
        &shims.join("python3"),
        "#!/bin/sh\nset -eu\ndir=target/speakers-analyze-runtime-link/linux-x86_64\nmkdir -p \"$dir\"\n: > \"$dir/libonnxruntime.so.1.25.0\"\n: > \"$dir/libonnxruntime.so.1\"\n: > \"$dir/libonnxruntime.so\"\nexit 23\n",
    );
    let output = Command::new("make")
        .arg("check-rust-onnx-stage")
        .current_dir(&temp.path)
        .env("PATH", fixture_path(&shims))
        .output()
        .expect("run failed staging fixture");
    assert!(
        !output.status.success(),
        "failed staging process was masked"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("failed to stage the pinned host ONNX Runtime")
    );
}

#[test]
fn every_linux_runtime_link_is_required_by_the_real_consumer() {
    let cases = [
        (
            "x86_64",
            "linux-x86_64",
            [
                "libonnxruntime.so.1.25.0",
                "libonnxruntime.so.1",
                "libonnxruntime.so",
            ],
        ),
        (
            "aarch64",
            "linux-aarch64",
            [
                "libonnxruntime.so.1.25.0",
                "libonnxruntime.so.1",
                "libonnxruntime.so",
            ],
        ),
    ];
    for (arch, target, names) in cases {
        for victim in names {
            for state in ["missing", "corrupt"] {
                let temp = TempDir::new("linux-onnx-consumer-negative");
                write_host_makefile(&temp.path, "Linux", arch);
                let shims = temp.path.join("shims");
                fs::create_dir(&shims).expect("create shims");
                write_native_cargo_shim(&shims.join("cargo"));
                write_executable(&shims.join("uname"), "#!/bin/sh\nprintf '%s\\n' Linux\n");
                let link_dir = seed_runtime(&temp.path, target, &names, &[]);
                if state == "missing" {
                    fs::remove_file(link_dir.join(victim)).expect("remove runtime fixture");
                } else {
                    fs::write(link_dir.join(victim), b"corrupt").expect("corrupt runtime fixture");
                }
                let argv_log = temp.path.join("cargo.argv");
                let output = Command::new("make")
                    .arg("check-rust-onnx-test")
                    .current_dir(&temp.path)
                    .env("PATH", fixture_path(&shims))
                    .env("SOLSTONE_CARGO_ARGV", &argv_log)
                    .env("SOLSTONE_CARGO_ENV", temp.path.join("cargo.env"))
                    .output()
                    .expect("run Linux ONNX consumer negative");
                assert!(
                    !output.status.success(),
                    "{target}/{victim} {state} greened"
                );
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(
                    stderr.contains(victim),
                    "diagnostic omitted {victim}: {stderr}"
                );
                assert!(stderr.contains("make check-rust-onnx-stage"));
                assert!(
                    !argv_log.exists(),
                    "Cargo ran with {target}/{victim} {state}"
                );
            }
        }
    }
}

#[test]
fn checksum_verifier_must_prove_itself_before_runtime_data_is_judged() {
    for (body, expected) in [
        ("exit 19", "failed its known-input check"),
        (
            "printf '%s\\n' not-a-digest",
            "failed its known-input check",
        ),
    ] {
        let temp = TempDir::new("onnx-verifier-negative");
        write_host_makefile(&temp.path, "Linux", "x86_64");
        let shims = temp.path.join("shims");
        fs::create_dir(&shims).expect("create shims");
        let verifier = shims.join("checksum");
        write_executable(&verifier, &format!("#!/bin/sh\n{body}\n"));
        replace_linux_fixture_verifier(&temp.path, &verifier);
        let python_sentinel = temp.path.join("python-ran");
        write_executable(
            &shims.join("python3"),
            "#!/bin/sh\n: > \"$SOLSTONE_PYTHON_SENTINEL\"\nexit 97\n",
        );
        let output = Command::new("make")
            .arg("check-rust-onnx-stage")
            .current_dir(&temp.path)
            .env("PATH", fixture_path(&shims))
            .env("SOLSTONE_PYTHON_SENTINEL", &python_sentinel)
            .output()
            .expect("run checksum verifier negative");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "wrong verifier diagnostic: {stderr}"
        );
        assert!(stderr.contains("install or repair"));
        assert!(!python_sentinel.exists(), "broken verifier invoked Python");
    }
}

#[test]
fn checksum_input_failure_is_not_misreported_as_a_broken_verifier() {
    let temp = TempDir::new("onnx-checksum-input-failure");
    write_host_makefile(&temp.path, "Linux", "x86_64");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create shims");
    let verifier = shims.join("checksum");
    write_executable(
        &verifier,
        "#!/bin/sh\nset -eu\nlast=\nfor argument in \"$@\"; do last=$argument; done\nprintf '%s\\n' \"$last\" >> \"$SOLSTONE_HASH_LOG\"\ncase \"$last\" in\n  *solstone-onnx-hash-probe-*) printf '1629c6bcea388b9f721343d214f545f712c0bc70ed9f34866a08b0f8ccb2edb7  %s\\n' \"$last\" ;;\n  *) exit 31 ;;\nesac\n",
    );
    replace_linux_fixture_verifier(&temp.path, &verifier);
    let names = [
        "libonnxruntime.so.1.25.0",
        "libonnxruntime.so.1",
        "libonnxruntime.so",
    ];
    seed_runtime(&temp.path, "linux-x86_64", &names, &[]);
    let hash_log = temp.path.join("hash.log");
    let python_sentinel = temp.path.join("python-ran");
    write_executable(
        &shims.join("python3"),
        "#!/bin/sh\n: > \"$SOLSTONE_PYTHON_SENTINEL\"\nexit 97\n",
    );
    let output = Command::new("make")
        .arg("check-rust-onnx-stage")
        .current_dir(&temp.path)
        .env("PATH", fixture_path(&shims))
        .env("SOLSTONE_HASH_LOG", &hash_log)
        .env("SOLSTONE_PYTHON_SENTINEL", &python_sentinel)
        .output()
        .expect("run checksum input failure fixture");
    assert!(!output.status.success());
    let calls = fs::read_to_string(hash_log).expect("read checksum call order");
    let calls = calls.lines().collect::<Vec<_>>();
    assert_eq!(calls.len(), 2, "unexpected checksum call order: {calls:?}");
    assert!(calls[0].contains("solstone-onnx-hash-probe-"));
    assert!(calls[1].ends_with(names[0]));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(names[0]));
    assert!(stderr.contains("checksum input failure"));
    assert!(stderr.contains("make check-rust-onnx-stage"));
    assert!(!stderr.contains("install or repair that verifier"));
    assert!(
        python_sentinel.exists(),
        "repairable input failure skipped staging"
    );
}

#[test]
fn supported_linux_skips_the_macos_gate_before_tool_requirements() {
    let temp = TempDir::new("linux-macos-skip");
    write_host_makefile(&temp.path, "Linux", "x86_64");
    let output = Command::new("/usr/bin/make")
        .arg("check-rust-macos")
        .current_dir(&temp.path)
        .env("PATH", "/path-with-no-tools")
        .output()
        .expect("run Linux macOS-gate skip");
    assert!(
        output.status.success(),
        "Linux skip reached a tool requirement: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("not run on Linux/x86_64"));
}

#[test]
fn unsupported_host_fails_only_when_a_host_gate_is_selected() {
    let temp = TempDir::new("unsupported-host");
    write_host_makefile(&temp.path, "Plan9", "mips");
    let unrelated = Command::new("make")
        .args(["-n", "check-rust-fmt"])
        .current_dir(&temp.path)
        .output()
        .expect("dry-run unrelated target");
    assert!(
        unrelated.status.success(),
        "unsupported host broke Make parsing"
    );

    let output = Command::new("make")
        .arg("check-rust-macos")
        .current_dir(&temp.path)
        .output()
        .expect("run unsupported host gate");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Plan9/mips"));
    assert!(stderr.contains("supported"));
}

#[test]
fn differential_gate_requires_validated_onnx_staging() {
    let makefile = makefile_text(&repo_root());
    let header = target_body(&makefile, "check-differentials")
        .lines()
        .next()
        .expect("check-differentials header");
    assert!(
        header.contains("check-rust-onnx-stage"),
        "check-differentials can bypass validated ONNX staging"
    );
    assert!(
        !target_body(&makefile, "ci-under-poison").contains("check-rust-onnx-stage"),
        "make ci must not invoke Python-backed staging"
    );
    assert!(
        !target_body(&makefile, "ci-full-under-poison").contains("check-rust-onnx-stage"),
        "make ci-full must not invoke Python-backed staging"
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
