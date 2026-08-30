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
fn make_ci_full_checks_dependency_policy_before_the_long_workspace_suite() {
    let root = repo_root();
    let registry = ci_registry(&root);
    let deny = registry
        .legs
        .iter()
        .position(|leg| leg.make_target == "check-rust-deny")
        .expect("registry must retain the dependency-policy gate");
    let workspace_tests = registry
        .legs
        .iter()
        .position(|leg| leg.id == "lib-bin")
        .expect("registry must retain the library/binary workspace gate");

    assert!(
        deny < workspace_tests,
        "dependency policy must run before the long workspace suite"
    );
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
fn make_ci_full_serializes_workspace_tests_that_compete_for_host_resources() {
    let makefile = makefile_text(&repo_root());
    assert!(
        makefile.contains("cargo test --manifest-path $(RUST_MANIFEST) --locked --offline \"$$@\" -- --test-threads=1"),
        "registry entries must serialize their individual test harnesses"
    );
    assert!(
        target_body(&makefile, "check-rust-unit").contains("-- --test-threads=1"),
        "the library/binary workspace leg must remain serialized"
    );
    assert!(
        ci_registry(&repo_root())
            .legs
            .iter()
            .any(|leg| leg.make_target == "check-rust-doc" && leg.default_full),
        "the default full registry must preserve doctests from the former workspace cargo test"
    );
    assert!(
        target_body(&makefile, "check-rust-doc").contains("--doc"),
        "the doctest leg must select Rust documentation tests explicitly"
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
        "check-rust-doc",
        "check-rust-test",
        "check-rust-describe-cli-stubs",
        "check-rust-onnx-test",
        "check-rust-pdf-test",
        "check-rust-shipped-binaries",
        "check-rust-ios",
        "check-rust-macos",
        "check-rust-windows",
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
        "$(RUST_ROUTINE_EXCLUDES)",
        "--lib",
        "--bins",
        "--locked",
        "--offline",
        "--no-fail-fast",
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
fn public_code_commands_keep_the_unit_boundary_and_report_it_from_make_variables() {
    let root = repo_root();
    let makefile = makefile_text(&root);
    let test = target_body(&makefile, "test");
    assert!(test.contains("check-rust-unit"));
    assert!(test.contains("report-rust-code-evidence"));
    assert!(test.find("check-rust-unit") < test.find("report-rust-code-evidence"));
    for forbidden in ["check-rust-test", "ci-full"] {
        assert!(!test.contains(forbidden));
    }
    let ci = target_body(&makefile, "ci-under-poison");
    assert!(ci.contains("$(MAKE) check-rust-unit"));
    assert!(
        ci.contains("report-rust-code-evidence") && ci.contains("RUST_CODE_EVIDENCE_CONTEXT=ci")
    );
    let report = target_body(&makefile, "report-rust-code-evidence");
    for required in [
        "$(RUST_ROUTINE_EXCLUDES)",
        "$(RUST_HOST_EXCLUDES)",
        "$(filter-out",
    ] {
        assert!(report.contains(required));
    }
    assert!(
        makefile.contains("RUST_CODE_EVIDENCE_CONTEXT")
            && makefile.contains("RUST_CODE_EVIDENCE_VALID")
    );
    assert!(
        report.contains("$(RUST_CODE_EVIDENCE_CLIPPY)")
            && report.contains("RUST_CODE_EVIDENCE_VALID")
    );
    assert!(!report.contains("solstone-core-"));
    let workspace =
        fs::read_to_string(root.join("core/Cargo.toml")).expect("read workspace Cargo manifest");
    assert!(!workspace_members(&workspace).is_empty());
}

#[test]
fn routine_slow_exclusions_are_exact_and_default_in_the_full_gate() {
    let makefile = makefile_text(&repo_root());
    let routine = makefile
        .lines()
        .find_map(|line| line.strip_prefix("RUST_ROUTINE_EXCLUDES := "))
        .expect("RUST_ROUTINE_EXCLUDES must be defined");
    assert!(routine.starts_with("$(RUST_HOST_EXCLUDES) "));
    let excluded = routine
        .split_whitespace()
        .skip(1)
        .collect::<Vec<_>>()
        .chunks_exact(2)
        .map(|pair| {
            assert_eq!(pair[0], "--exclude");
            pair[1]
        })
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "solstone-core-convey-body",
        "solstone-core-describe",
        "solstone-core-facets",
        "solstone-core-sol-link",
    ]);
    assert_eq!(excluded, expected, "routine-only exclusions drifted");

    let registry = ci_registry(&repo_root());
    for package in expected {
        let suite = registry
            .package_suites
            .iter()
            .find(|suite| suite.package == package)
            .unwrap_or_else(|| panic!("{package} has no full package suite"));
        assert!(
            suite.default_full,
            "{package} is excluded from routine CI but not default full"
        );
    }
}

#[test]
fn efficient_ci_statically_checks_only_library_and_binary_targets() {
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
        "--lib",
        "--bins",
        "--locked",
        "--offline",
        "-- -D warnings",
    ] {
        assert!(
            invocation.contains(required),
            "efficient CI lost routine static-compilation coverage: {required}"
        );
    }
    assert!(
        !invocation.contains("--all-targets"),
        "routine CI must not compile integration targets"
    );

    let full = target_body(&makefile, "check-rust-clippy-full");
    assert!(
        full.contains("--all-targets") && full.contains("-- -D warnings"),
        "full CI must retain all-target static compilation"
    );
}

#[test]
fn routine_ci_is_fail_closed_and_contained_on_linux() {
    let makefile = makefile_text(&repo_root());
    let ci = target_body(&makefile, "ci");
    for required in [
        "command -v bwrap",
        "mktemp -d /var/tmp/solstone-ci-XXXXXX",
        "trap 'rm -rf -- \"$$sandbox_root\"'",
        "--unshare-net",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--bind \"$$sandbox_root/tmp\" /tmp",
        "--ro-bind \"$$sandbox_root/empty\" /run",
        "--bind \"$$sandbox_root/var-tmp\" /var/tmp",
        "--ro-bind \"$(CURDIR)\" \"$(CURDIR)\"",
        "--bind \"$(RUST_TARGET_DIR)\" \"$(RUST_TARGET_DIR)\"",
        "--setenv CARGO_NET_OFFLINE true",
        "$(MAKE) --no-print-directory ci-contained",
    ] {
        assert!(
            ci.contains(required),
            "make ci lost containment: {required}"
        );
    }
    assert!(
        !ci.contains("--tmpfs"),
        "routine CI temporary storage must remain disk-backed under /var/tmp"
    );
    assert!(
        makefile.contains(
            "ci ci-contained ci-under-poison ci-prep-ffmpeg: export SOLSTONE_FFMPEG_SOURCE_ARCHIVE := $(FFMPEG_SOURCE_ARCHIVE)"
        ),
        "routine CI must use the prepared pinned FFmpeg source archive"
    );
    assert!(
        makefile
            .contains("ci ci-contained ci-under-poison ci-prep-ffmpeg: export SOLSTONE_DISTRIBUTION_OFFLINE := 1"),
        "routine CI must fail closed instead of fetching FFmpeg inside containment"
    );
    assert!(
        makefile.contains("ci: ci-prep-ffmpeg"),
        "routine CI must acquire its pinned FFmpeg archive before containment"
    );
    let ffmpeg_prep = target_body(&makefile, "ci-prep-ffmpeg");
    assert!(
        ffmpeg_prep.contains("acquire ffmpeg")
            && ffmpeg_prep.contains("--dest $(FFMPEG_SOURCE_ARCHIVE)"),
        "routine CI prep must materialize the pinned FFmpeg archive"
    );
    assert!(
        target_body(&makefile, "ci-contained").contains("SOLSTONE_CI_CONTAINED"),
        "the contained entry point must reject direct use"
    );
}

#[test]
fn default_full_runs_the_describe_cli_stub_matrix_once() {
    let registry = ci_registry(&repo_root());
    let leg = registry
        .legs
        .iter()
        .find(|leg| leg.id == "describe-stubs")
        .expect("registry must retain the describe-stubs leg");
    let suite = registry
        .suites
        .iter()
        .find(|suite| suite.id == "solstone-core-describe::cli")
        .expect("registry must retain the selectable describe CLI suite");

    assert!(
        leg.default_full,
        "the census-bearing stub leg must run by default"
    );
    assert_eq!(leg.make_target, "check-rust-describe-cli-stubs");
    assert_eq!(suite.required_features, ["test-stubs"]);
    assert!(
        !suite.default_full,
        "the describe CLI matrix must not run twice in default full CI"
    );
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
fn distribution_poison_does_not_forbid_native_tools_in_canonical_ci() {
    let makefile = makefile_text(&repo_root());
    assert!(
        makefile.contains("CI_FORBIDDEN_INTERPRETERS := python python3 pytest ruff uv\n"),
        "canonical CI poison must remain limited to interpreters"
    );
    let distribution_tools = makefile
        .lines()
        .find(|line| line.starts_with("DISTRIBUTION_FORBIDDEN_TOOLS :="))
        .expect("distribution must declare its expanded producer-tool poison");
    for tool in [
        "$(CI_FORBIDDEN_INTERPRETERS)",
        "maturin",
        "dpkg-deb",
        "rpmbuild",
        "ar",
        "rpm",
        "tar",
        "cpio",
        "curl",
        "wget",
    ] {
        assert!(
            distribution_tools
                .split_whitespace()
                .any(|item| item == tool),
            "distribution poison lost {tool}"
        );
    }
    assert!(
        target_body(&makefile, "check-rust-distribution")
            .contains("$(DISTRIBUTION_FORBIDDEN_TOOLS)"),
        "distribution gate must select its expanded tool poison"
    );
    assert!(
        makefile.contains("$(if $(strip $(2)),$(2),$(CI_FORBIDDEN_INTERPRETERS))"),
        "shared poison wrapper must honor the distribution gate's override"
    );
}

#[test]
fn make_ci_full_names_the_manual_rust_race_gate() {
    let registry = ci_registry(&repo_root());
    let race = registry
        .legs
        .iter()
        .find(|leg| leg.set == "race")
        .expect("registry must explicitly name the race lane");
    assert_eq!(race.make_target, "check-rust-race");
    assert!(
        !race.default_full,
        "race must remain outside default full CI"
    );
}

#[test]
fn make_ci_full_keeps_apple_gates_native_to_apple_sdk_hosts() {
    let root = repo_root();
    let makefile = makefile_text(&root);
    let registry = ci_registry(&root);
    let ios = target_body(&makefile, "check-rust-ios");
    let macos = target_body(&makefile, "check-rust-macos");

    assert!(
        registry
            .legs
            .iter()
            .any(|leg| leg.make_target == "check-rust-ios" && leg.default_full),
        "the default full registry must retain the iOS gate"
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
        registry
            .legs
            .iter()
            .any(|leg| leg.make_target == "check-rust-macos" && leg.default_full),
        "the default full registry must retain the macOS cfg gate"
    );
    for protected in [
        "cargo test",
        "--manifest-path core/Cargo.toml",
        "--workspace",
        "--all-targets",
        "--no-run",
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

#[test]
fn make_ci_full_keeps_the_windows_crosscheck_fail_closed() {
    let root = repo_root();
    let makefile = makefile_text(&root);
    let windows = target_body(&makefile, "check-rust-windows");
    assert!(
        ci_registry(&root)
            .legs
            .iter()
            .any(|leg| leg.make_target == "check-rust-windows" && leg.default_full),
        "the default full registry must retain the Windows cross-check"
    );
    for protected in [
        "$(REQUIRE_RUSTUP)",
        "$(WINDOWS_TARGET)",
        "solstone-windows-crosscheck",
        "--locked",
        "--offline",
        "core/ci/windows-crosscheck.toml",
    ] {
        assert!(
            windows.contains(protected),
            "check-rust-windows lost its fail-closed contract: {protected}"
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
fn full_ci_stages_host_runtimes_before_entering_the_poisoned_gate() {
    let makefile = makefile_text(&repo_root());
    let pdf_stager = fs::read_to_string(
        repo_root().join("core/crates/solstone-core-distribution/src/pdfium.rs"),
    )
    .expect("read PDFium staging source");
    let prep = target_body(&makefile, "ci-full-prep");
    for (stage, prep_target) in [
        ("check-rust-onnx-stage", "ci-full-prep-onnx"),
        ("check-rust-pdf-stage", "ci-full-prep-pdf"),
    ] {
        assert!(
            prep.contains(prep_target),
            "make ci-full-prep must retain the dedicated prep lane for {stage}"
        );
    }
    for gate in ["ci-under-poison", "ci-full", "ci-full-under-poison"] {
        let body = target_body(&makefile, gate);
        assert!(
            !body.contains("check-rust-onnx-stage") && !body.contains("check-rust-pdf-stage"),
            "{gate} must not invoke runtime staging"
        );
    }
    assert!(target_body(&makefile, "ci-full-prep-onnx").contains("check-rust-onnx-stage"));
    assert!(target_body(&makefile, "ci-full-prep-pdf").contains("check-rust-pdf-stage"));
    assert!(
        makefile.contains(
            "REQUIRE_PDF_HOST_RUNTIME = $(REQUIRE_SUPPORTED_PDF_HOST); $(DEFINE_PDF_RUNTIME_VALIDATOR);"
        ),
        "PDF runtime readiness must invoke the validator rather than expand to an empty command"
    );
    assert!(
        !makefile.contains("--package-dir packages/")
            && makefile.contains("--package-dir target/runtime-package-staging/"),
        "runtime prep must keep generated package-shaped staging under target/"
    );
    let pdf_stage = target_body(&makefile, "check-rust-pdf-stage");
    assert!(pdf_stage.contains("acquire pdfium"));
    assert!(!pdf_stage.contains("python3"));
    assert!(pdf_stage.contains("REQUIRE_PDF_HOST_RUNTIME"));
    assert!(target_body(&makefile, "check-rust-pdf-ready").contains("REQUIRE_PDF_HOST_RUNTIME"));
    assert!(target_body(&makefile, "check-rust-onnx-stage").contains("acquire onnx"));
    assert!(!target_body(&makefile, "check-rust-onnx-stage").contains("python3"));
    for digest in [
        "687dce861f959c7097d47c5864509d51a926a71b38322596a8ee3e7a99c6b96e",
        "933f3d620cc8b58fb30a7f12a1bce8bf276da65caf39ff8fb2d04bc1268d53a3",
        "df568fcd17a6a6296956aa79abea1181db187458432f360b084fec1cea7cd4d9",
    ] {
        assert!(
            makefile.contains(digest),
            "Makefile omitted PDFium digest {digest}"
        );
        assert!(
            pdf_stager.contains(digest),
            "PDFium staging source omitted Makefile digest {digest}"
        );
    }
    assert!(
        target_body(&makefile, "ci-full-prep-cargo").contains("cargo fetch"),
        "full prep must own Cargo fetching"
    );
    let cargo_prep = target_body(&makefile, "ci-full-prep-cargo");
    assert!(
        !cargo_prep.contains("python3")
            && cargo_prep.contains("cargo check")
            && cargo_prep.contains("$(RUST_HOST_EXCLUDES)")
            && cargo_prep.contains("cargo test")
            && cargo_prep.contains("$(RUST_ROUTINE_EXCLUDES)")
            && cargo_prep.contains("--no-run"),
        "full prep must materialize both routine static and unit build graphs"
    );
    assert!(
        makefile.contains("ci-full-prep-cargo: ci-prep-ffmpeg"),
        "full Cargo prep must share the pinned FFmpeg acquisition step"
    );
    assert!(
        makefile.contains(
            "ci-full ci-full-under-poison ci-full-prep ci-full-prep-cargo: export SOLSTONE_DISTRIBUTION_OFFLINE := 1"
        ),
        "full validation and prep must make the pinned FFmpeg archive mandatory"
    );
    assert!(
        !target_body(&makefile, "check-rust-deny").contains("cargo fetch"),
        "offline validation must not fetch Cargo inputs"
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
        "check-rust-clippy-full",
        "check-rust-doc",
        "check-rust-test",
    ] {
        assert!(
            target_body(&makefile, target).contains("$(RUST_HOST_EXCLUDES)"),
            "{target} must use RUST_HOST_EXCLUDES"
        );
    }
    assert!(
        target_body(&makefile, "check-rust-unit").contains("$(RUST_ROUTINE_EXCLUDES)"),
        "check-rust-unit must use the routine-only exclusion set"
    );
}
