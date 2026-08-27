// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

include!("support/repository_make_command_graphs.rs");

const SANDBOX_RUNTIME_NAMES: [&str; 3] = [
    "libonnxruntime.so.1.25.0",
    "libonnxruntime.so.1",
    "libonnxruntime.so",
];

fn seed_sandbox_payload(target_dir: &Path) -> PathBuf {
    let payload_dir = target_dir.join("lib/solstone-core-speakers-analyze");
    fs::create_dir_all(&payload_dir).expect("create sandbox processing payload directory");
    for name in SANDBOX_RUNTIME_NAMES {
        fs::write(payload_dir.join(name), []).expect("write sandbox processing runtime fixture");
    }
    payload_dir
}

fn write_sandbox_processing_helpers(target_dir: &Path) {
    let helper_dir = target_dir.join("debug");
    fs::create_dir_all(&helper_dir).expect("create sandbox processing helper directory");
    for (name, schema) in [
        (
            "solstone-core-speakers-analyze",
            "solstone-speaker-analyze-error-v1",
        ),
        ("solstone-core-vad-analyze", "solstone-vad-error-v1"),
    ] {
        write_executable(
            &helper_dir.join(name),
            &format!(
                "#!/bin/sh\nset -eu\nprintf '%s LD=%s DYLD=%s\\n' '{name}' \"${{LD_LIBRARY_PATH-unset}}\" \"${{DYLD_LIBRARY_PATH-unset}}\" >> \"$SOLSTONE_HELPER_ENV_LOG\"\nprintf '%s\\n' '{{\"schema\":\"{schema}\",\"reason\":\"malformed-request\"}}' >&2\nexit 64\n"
            ),
        );
    }
}

fn fixture_make_command(root: &Path, shims: &Path, target: &str) -> Command {
    let mut command = Command::new("make");
    command
        .arg(target)
        .current_dir(root)
        .env("PATH", fixture_path(shims));
    command
}

#[test]
fn make_ci_never_executes_forbidden_interpreters() {
    assert_gate_never_executes_forbidden_interpreters(
        "ci",
        &["run", "fmt", "run", "clippy", "test"],
    );
}

#[test]
fn make_ci_full_never_executes_forbidden_interpreters() {
    // The Cargo shim records the runner launch. Per-entry traversal is owned by
    // the runner and covered by its selector, poison, and command tests.
    assert_gate_never_executes_forbidden_interpreters("ci-full", &["run"]);
}

#[test]
fn make_clean_reclaims_default_and_configured_cargo_targets() {
    let temp = TempDir::new("make-clean-cargo-targets");
    let root = &temp.path;
    let system = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    let arch = String::from_utf8(
        Command::new("/usr/bin/uname")
            .arg("-m")
            .output()
            .expect("inspect fixture host architecture")
            .stdout,
    )
    .expect("host architecture is UTF-8");
    write_host_makefile(root, system, arch.trim());

    let default_target = root.join("core/target");
    fs::create_dir_all(&default_target).expect("create default Cargo target fixture");
    fs::write(default_target.join("sentinel"), b"default").expect("seed default target fixture");
    let default_clean = Command::new("make")
        .arg("clean")
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(root)
        .output()
        .expect("run make clean for default Cargo target");
    assert!(
        default_clean.status.success(),
        "make clean failed for default Cargo target: {}",
        String::from_utf8_lossy(&default_clean.stderr)
    );
    assert!(
        !default_target.exists(),
        "make clean left the default core/target directory behind"
    );

    let configured_target = root.join("private-cargo-target");
    fs::create_dir_all(&configured_target).expect("create configured Cargo target fixture");
    fs::write(configured_target.join("sentinel"), b"configured")
        .expect("seed configured target fixture");
    let configured_clean = Command::new("make")
        .arg("clean")
        .env("CARGO_TARGET_DIR", &configured_target)
        .current_dir(root)
        .output()
        .expect("run make clean for configured Cargo target");
    assert!(
        configured_clean.status.success(),
        "make clean failed for configured Cargo target: {}",
        String::from_utf8_lossy(&configured_clean.stderr)
    );
    assert!(
        !configured_target.exists(),
        "make clean left the configured Cargo target directory behind"
    );
}

#[test]
fn ci_entrypoints_override_hostile_cargo_disk_settings() {
    #[derive(Clone, Copy, Debug)]
    enum OverrideChannel {
        Environment,
        EnvironmentWins,
        CommandLine,
        Makeflags,
    }

    for gate in ["ci", "ci-full", "ci-full-prep", "ci-full-prep-cargo"] {
        for channel in [
            OverrideChannel::Environment,
            OverrideChannel::EnvironmentWins,
            OverrideChannel::CommandLine,
            OverrideChannel::Makeflags,
        ] {
            let temp = TempDir::new("ci-cargo-disk-settings");
            let root = &temp.path;
            let system = if cfg!(target_os = "macos") {
                "Darwin"
            } else {
                "Linux"
            };
            let arch = String::from_utf8(
                Command::new("/usr/bin/uname")
                    .arg("-m")
                    .output()
                    .expect("inspect fixture host architecture")
                    .stdout,
            )
            .expect("host architecture is UTF-8");
            write_host_makefile(root, system, arch.trim());
            let runtime_target = if cfg!(target_os = "macos") {
                "macos-arm64"
            } else if matches!(arch.trim(), "aarch64" | "arm64") {
                "linux-aarch64"
            } else {
                "linux-x86_64"
            };
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
            seed_runtime(root, runtime_target, onnx_names, &[]);
            let pdf_name = if cfg!(target_os = "macos") {
                "libpdfium.dylib"
            } else {
                "libpdfium.so"
            };
            let pdf_dir = root.join("target/pdfium-runtime-link").join(runtime_target);
            fs::create_dir_all(&pdf_dir).expect("create PDF runtime fixture directory");
            fs::write(pdf_dir.join(pdf_name), []).expect("write PDF runtime fixture");

            let shims = root.join("shims");
            let target = root.join("private-cargo-target");
            let cargo_log = target.join("cargo.log");
            let environment_log = target.join("cargo.env");
            fs::create_dir(&shims).expect("create Cargo shim directory");
            fs::create_dir(&target).expect("create configured Cargo target");
            write_recording_cargo_shim(&shims.join("cargo"));

            let mut command = Command::new("make");
            command
                .arg(gate)
                .current_dir(root)
                .env("PATH", fixture_path(&shims))
                .env("CARGO_TARGET_DIR", &target)
                .env("SOLSTONE_CI_CARGO_LOG", &cargo_log)
                .env("SOLSTONE_CI_CARGO_ENV_LOG", &environment_log);
            match channel {
                OverrideChannel::Environment => {
                    command
                        .env("CARGO_INCREMENTAL", "1")
                        .env("CARGO_PROFILE_DEV_DEBUG", "2");
                }
                OverrideChannel::EnvironmentWins => {
                    command
                        .arg("-e")
                        .env("CARGO_INCREMENTAL", "1")
                        .env("CARGO_PROFILE_DEV_DEBUG", "2");
                }
                OverrideChannel::CommandLine => {
                    command.args(["CARGO_INCREMENTAL=1", "CARGO_PROFILE_DEV_DEBUG=2"]);
                }
                OverrideChannel::Makeflags => {
                    command.env("MAKEFLAGS", "CARGO_INCREMENTAL=1 CARGO_PROFILE_DEV_DEBUG=2");
                }
            }
            let output = command.output().expect("run disk-lean CI fixture");
            assert!(
                output.status.success(),
                "make {gate} failed under {channel:?} Cargo settings:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );

            let expected = format!("0|0|{}", target.display());
            let observed = fs::read_to_string(&environment_log)
                .expect("CI entry point must execute a Cargo-backed command");
            assert!(
                observed.lines().next().is_some(),
                "make {gate} under {channel:?} did not record a Cargo invocation"
            );
            assert!(
                observed.lines().all(|line| line == expected),
                "make {gate} under {channel:?} leaked hostile Cargo settings or lost the configured target:\n{observed}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn msrv_target_is_owned_and_reclaimed_on_success_and_failure() {
    for cargo_exit in [0, 23] {
        let temp = TempDir::new("msrv-owned-target");
        let root = &temp.path;
        write_host_makefile(root, "Linux", "x86_64");
        let shims = root.join("shims");
        fs::create_dir(&shims).expect("create shim fixture directory");
        write_executable(
            &shims.join("rustup"),
            "#!/bin/sh\nprintf '%s\\n' '1.95.0-x86_64-unknown-linux-gnu'\n",
        );
        write_executable(
            &shims.join("cargo"),
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$CARGO_TARGET_DIR\" > \"$SOLSTONE_MSRV_TARGET_LOG\"\nmkdir -p \"$CARGO_TARGET_DIR\"\nprintf '%s' owned > \"$CARGO_TARGET_DIR/owned-sentinel\"\nexit \"$SOLSTONE_CARGO_EXIT\"\n",
        );

        let configured_target = root.join("private-cargo-target");
        fs::create_dir(&configured_target).expect("create configured Cargo target");
        let unrelated = configured_target.join("unrelated-sentinel");
        fs::write(&unrelated, b"preserve me").expect("write unrelated target sentinel");
        let target_log = root.join("msrv-target.log");
        let output = Command::new("make")
            .arg("check-rust-msrv")
            .current_dir(root)
            .env("PATH", fixture_path(&shims))
            .env("CARGO_TARGET_DIR", &configured_target)
            .env("SOLSTONE_CARGO_EXIT", cargo_exit.to_string())
            .env("SOLSTONE_MSRV_TARGET_LOG", &target_log)
            .output()
            .expect("run isolated MSRV fixture");
        assert_eq!(
            output.status.success(),
            cargo_exit == 0,
            "MSRV gate changed Cargo's status:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let owned_target = PathBuf::from(
            fs::read_to_string(&target_log)
                .expect("read isolated MSRV target")
                .trim(),
        );
        assert!(owned_target.starts_with(&configured_target));
        assert!(
            owned_target
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("ci-msrv-1.95.0-"))
        );
        assert!(
            !owned_target.exists(),
            "isolated MSRV target survived after Cargo exited"
        );
        assert_eq!(
            fs::read(&unrelated).expect("read unrelated target sentinel"),
            b"preserve me"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn msrv_target_is_retained_while_a_descendant_still_uses_it() {
    let temp = TempDir::new("msrv-live-descendant");
    let root = &temp.path;
    write_host_makefile(root, "Linux", "x86_64");
    let shims = root.join("shims");
    fs::create_dir(&shims).expect("create shim fixture directory");
    write_executable(
        &shims.join("rustup"),
        "#!/bin/sh\nprintf '%s\\n' '1.95.0-x86_64-unknown-linux-gnu'\n",
    );
    write_executable(
        &shims.join("cargo"),
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$CARGO_TARGET_DIR\" > \"$SOLSTONE_MSRV_TARGET_LOG\"\nmkdir -p \"$CARGO_TARGET_DIR\"\n/bin/sh -c 'printf \"%s\\n\" \"$$\" > \"$1\"; : > \"$2\"; exec /bin/sleep 30' child \"$SOLSTONE_CHILD_PID_LOG\" \"$SOLSTONE_CHILD_READY\" </dev/null >/dev/null 2>&1 &\nwhile [ ! -e \"$SOLSTONE_CHILD_READY\" ]; do /bin/sleep 0.01; done\nexit 23\n",
    );

    let configured_target = root.join("private-cargo-target");
    let target_log = root.join("msrv-target.log");
    let child_pid_log = root.join("child.pid");
    let child_ready = root.join("child.ready");
    let output = Command::new("make")
        .arg("check-rust-msrv")
        .current_dir(root)
        .env("PATH", fixture_path(&shims))
        .env("CARGO_TARGET_DIR", &configured_target)
        .env("SOLSTONE_MSRV_TARGET_LOG", &target_log)
        .env("SOLSTONE_CHILD_PID_LOG", &child_pid_log)
        .env("SOLSTONE_CHILD_READY", &child_ready)
        .output()
        .expect("run live-descendant MSRV fixture");
    assert!(!output.status.success(), "failing Cargo status was masked");
    let owned_target = PathBuf::from(
        fs::read_to_string(&target_log)
            .expect("read isolated MSRV target")
            .trim(),
    );
    assert!(
        owned_target.exists(),
        "MSRV target was removed while a child process used it"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("MSRV target retained"),
        "retention diagnostic was absent: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let child_pid = fs::read_to_string(&child_pid_log).expect("read child PID");
    let kill = Command::new("/bin/kill")
        .arg(child_pid.trim())
        .status()
        .expect("terminate exact fixture child");
    assert!(kill.success(), "failed to terminate exact fixture child");
    fs::remove_dir_all(&owned_target).expect("remove retained fixture target after child exit");
}

#[test]
fn manual_race_gate_is_selectable_without_uv() {
    let root = repo_root();
    let mut targets = ci_registry(&root)
        .legs
        .into_iter()
        .map(|leg| leg.make_target)
        .collect::<BTreeSet<_>>();
    targets.extend(
        [
            "ci",
            "ci-under-poison",
            "ci-full",
            "ci-full-under-poison",
            "ci-full-plan",
            "ci-full-prep",
            "ci-full-prep-cargo",
            "check-rust-ci-topology",
            "check-rust-registry-suite",
            "check-rust-registry-package",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    targets.insert(["ci-full-prep-", "on", "nx"].concat());
    targets.insert(["ci-full-prep-", "p", "df"].concat());
    targets.insert(["check-rust-", "on", "nx", "-ready"].concat());
    targets.insert(["check-rust-", "p", "df", "-ready"].concat());
    for target in targets {
        let dry_run = Command::new("make")
            .args(["-n", &target])
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&root)
            .output()
            .expect("uv-free make dry run starts");
        assert!(
            dry_run.status.success(),
            "{target} must be selectable without uv: {}",
            String::from_utf8_lossy(&dry_run.stderr)
        );
    }
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
        let acquire_log = temp.path.join("acquire.log");
        write_acquire_shim(
            &shims.join("cargo"),
            &format!(
                "    dir=target/speakers-analyze-runtime-link/linux-x86_64\n    mkdir -p \"$dir\"\n    : > \"$dir/libonnxruntime.so.1.25.0\"\n    : > \"$dir/libonnxruntime.so.1\"\n    : > \"$dir/libonnxruntime.so\"\n    {}\n    exit 0\n",
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
            .env("SOLSTONE_ACQUIRE_LOG", &acquire_log)
            .output()
            .expect("run staging postcondition negative");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(victim),
            "postvalidation omitted {victim}: {stderr}"
        );
        assert!(stderr.contains("make check-rust-onnx-stage"));
        let acquire_argv = nul_argv(&acquire_log);
        assert_eq!(
            acquire_argv.iter().filter(|arg| *arg == "--target").count(),
            1,
            "repair must invoke acquire onnx exactly once"
        );
        assert!(
            acquire_argv
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
    write_acquire_shim(
        &shims.join("cargo"),
        "    dir=target/speakers-analyze-runtime-link/linux-x86_64\n    mkdir -p \"$dir\"\n    : > \"$dir/libonnxruntime.so.1.25.0\"\n    : > \"$dir/libonnxruntime.so.1\"\n    : > \"$dir/libonnxruntime.so\"\n    exit 23\n",
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
fn sandbox_processing_build_uses_the_effective_target_and_replaces_the_payload() {
    for configured_target in [false, true] {
        let temp = TempDir::new("sandbox-processing-build");
        write_host_makefile(&temp.path, "Linux", "x86_64");
        let shims = temp.path.join("shims");
        fs::create_dir(&shims).expect("create Cargo shim directory");
        write_native_cargo_shim(&shims.join("cargo"));
        let source_dir = seed_runtime(&temp.path, "linux-x86_64", &SANDBOX_RUNTIME_NAMES, &[]);
        for name in SANDBOX_RUNTIME_NAMES.iter().skip(1) {
            fs::remove_file(source_dir.join(name)).expect("remove source runtime copy");
            std::os::unix::fs::symlink(SANDBOX_RUNTIME_NAMES[0], source_dir.join(name))
                .expect("create source runtime link");
        }

        let target_dir = if configured_target {
            temp.path.join("private-cargo-target")
        } else {
            temp.path.join("core/target")
        };
        let stale_dir = target_dir.join("lib/solstone-core-speakers-analyze");
        fs::create_dir_all(&stale_dir).expect("create stale runtime payload directory");
        fs::write(stale_dir.join("stale"), b"stale").expect("write stale runtime payload entry");
        let argv_log = temp.path.join("cargo.argv");
        let env_log = temp.path.join("cargo.env");
        let mut command = fixture_make_command(&temp.path, &shims, "build-sandbox-processing");
        command
            .env("SOLSTONE_CARGO_ARGV", &argv_log)
            .env("SOLSTONE_CARGO_ENV", &env_log);
        if configured_target {
            command.env("CARGO_TARGET_DIR", &target_dir);
        } else {
            command.env_remove("CARGO_TARGET_DIR");
        }
        let output = command
            .output()
            .expect("run sandbox processing build fixture");
        assert!(
            output.status.success(),
            "sandbox processing build failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let argv = nul_argv(&argv_log);
        assert_eq!(argv.first().map(String::as_str), Some("build"));
        assert!(
            argv.windows(2)
                .any(|pair| { pair == ["-p", "solstone-core-speakers-analyze"] })
        );
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["-p", "solstone-core-vad-analyze"])
        );
        let recorded_env = fs::read_to_string(&env_log).expect("read Cargo environment");
        assert!(recorded_env.contains("ORT_PREFER_DYNAMIC_LINK=true"));
        assert!(recorded_env.contains(&format!(
            "ORT_LIB_PATH={}",
            fs::canonicalize(&source_dir)
                .expect("canonical source runtime directory")
                .display()
        )));

        let payload_dir = target_dir.join("lib/solstone-core-speakers-analyze");
        assert!(!payload_dir.join("stale").exists());
        for name in SANDBOX_RUNTIME_NAMES {
            assert!(payload_dir.join(name).is_file());
            assert!(
                !fs::symlink_metadata(payload_dir.join(name))
                    .expect("inspect final runtime entry")
                    .file_type()
                    .is_symlink(),
                "final runtime entry {name} must be a regular file"
            );
        }
        for name in SANDBOX_RUNTIME_NAMES.iter().skip(1) {
            assert!(
                fs::symlink_metadata(source_dir.join(name))
                    .expect("inspect source runtime entry")
                    .file_type()
                    .is_symlink(),
                "source stage link {name} must remain accepted"
            );
        }
    }
}

#[test]
fn sandbox_processing_build_self_stages_missing_source_then_builds() {
    let temp = TempDir::new("sandbox-processing-self-staging");
    write_host_makefile(&temp.path, "Linux", "x86_64");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create Cargo shim directory");
    write_acquire_and_build_cargo_shim(
        &shims.join("cargo"),
        "    dir=target/speakers-analyze-runtime-link/linux-x86_64\n    mkdir -p \"$dir\"\n    : > \"$dir/libonnxruntime.so.1.25.0\"\n    : > \"$dir/libonnxruntime.so.1\"\n    : > \"$dir/libonnxruntime.so\"",
    );
    let acquire_log = temp.path.join("acquire.log");
    let argv_log = temp.path.join("cargo.argv");
    let env_log = temp.path.join("cargo.env");
    let output = fixture_make_command(&temp.path, &shims, "build-sandbox-processing")
        .env("SOLSTONE_ACQUIRE_LOG", &acquire_log)
        .env("SOLSTONE_CARGO_ARGV", &argv_log)
        .env("SOLSTONE_CARGO_ENV", &env_log)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("run sandbox processing self-staging build");
    assert!(
        output.status.success(),
        "sandbox processing self-staging build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let acquire_argv = nul_argv(&acquire_log);
    assert_eq!(
        acquire_argv.iter().filter(|arg| *arg == "--target").count(),
        1,
        "self-staging must invoke acquire onnx exactly once"
    );
    assert!(
        acquire_argv
            .windows(2)
            .any(|pair| pair == ["--target", "linux-x86_64"])
    );

    let argv = nul_argv(&argv_log);
    assert_eq!(argv.first().map(String::as_str), Some("build"));
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["-p", "solstone-core-speakers-analyze"])
    );
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["-p", "solstone-core-vad-analyze"])
    );
    let source_dir = temp
        .path
        .join("target/speakers-analyze-runtime-link/linux-x86_64");
    let recorded_env = fs::read_to_string(&env_log).expect("read Cargo environment");
    assert!(recorded_env.contains("ORT_PREFER_DYNAMIC_LINK=true"));
    assert!(recorded_env.contains(&format!(
        "ORT_LIB_PATH={}",
        fs::canonicalize(&source_dir)
            .expect("canonical source runtime directory")
            .display()
    )));

    let payload_dir = temp
        .path
        .join("core/target/lib/solstone-core-speakers-analyze");
    for name in SANDBOX_RUNTIME_NAMES {
        assert!(payload_dir.join(name).is_file());
        assert!(
            !fs::symlink_metadata(payload_dir.join(name))
                .expect("inspect final runtime entry")
                .file_type()
                .is_symlink(),
            "final runtime entry {name} must be a regular file"
        );
    }
}

#[test]
fn sandbox_processing_build_refuses_a_symlinked_target_library_directory() {
    let temp = TempDir::new("sandbox-processing-symlinked-library-directory");
    write_host_makefile(&temp.path, "Linux", "x86_64");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create Cargo shim directory");
    write_native_cargo_shim(&shims.join("cargo"));
    seed_runtime(&temp.path, "linux-x86_64", &SANDBOX_RUNTIME_NAMES, &[]);
    let target_dir = temp.path.join("core/target");
    fs::create_dir_all(&target_dir).expect("create target directory");
    let outside_dir = temp.path.join("outside-target");
    fs::create_dir(&outside_dir).expect("create outside directory");
    let sentinel = outside_dir.join("sentinel");
    fs::write(&sentinel, b"preserve me").expect("write outside sentinel");
    let target_library_dir = target_dir.join("lib");
    std::os::unix::fs::symlink(&outside_dir, &target_library_dir)
        .expect("link target library directory outside the target");
    let argv_log = temp.path.join("cargo.argv");
    let output = fixture_make_command(&temp.path, &shims, "build-sandbox-processing")
        .env("SOLSTONE_CARGO_ARGV", &argv_log)
        .env("SOLSTONE_CARGO_ENV", temp.path.join("cargo.env"))
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("run sandbox processing symlink negative");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains(target_library_dir.to_string_lossy().as_ref())
    );
    assert!(
        !argv_log.exists(),
        "symlinked target library directory invoked Cargo"
    );
    assert_eq!(
        fs::read(&sentinel).expect("read outside sentinel"),
        b"preserve me"
    );
}

#[test]
fn sandbox_processing_check_rejects_invalid_payload_before_helpers() {
    for state in ["missing", "corrupt", "symlink"] {
        let temp = TempDir::new(&format!("sandbox-processing-payload-{state}"));
        write_host_makefile(&temp.path, "Linux", "x86_64");
        let shims = temp.path.join("shims");
        fs::create_dir(&shims).expect("create Cargo shim directory");
        write_acquire_and_build_cargo_shim(&shims.join("cargo"), "");
        let target_dir = temp.path.join("core/target");
        let payload_dir = if state == "missing" {
            target_dir.join("lib/solstone-core-speakers-analyze")
        } else {
            seed_sandbox_payload(&target_dir)
        };
        if state == "corrupt" {
            fs::write(payload_dir.join(SANDBOX_RUNTIME_NAMES[1]), b"corrupt")
                .expect("corrupt final runtime entry");
        } else if state == "symlink" {
            fs::remove_file(payload_dir.join(SANDBOX_RUNTIME_NAMES[1]))
                .expect("remove final runtime entry");
            std::os::unix::fs::symlink(
                SANDBOX_RUNTIME_NAMES[0],
                payload_dir.join(SANDBOX_RUNTIME_NAMES[1]),
            )
            .expect("link final runtime entry");
        }
        write_sandbox_processing_helpers(&target_dir);
        let acquire_log = temp.path.join("acquire.log");
        let argv_log = temp.path.join("cargo.argv");
        let helper_env_log = temp.path.join("helper.env");
        let output =
            fixture_make_command(&temp.path, &shims, "check-rust-sandbox-processing-build")
                .env("SOLSTONE_ACQUIRE_LOG", &acquire_log)
                .env("SOLSTONE_CARGO_ARGV", &argv_log)
                .env("SOLSTONE_CARGO_ENV", temp.path.join("cargo.env"))
                .env("SOLSTONE_HELPER_ENV_LOG", &helper_env_log)
                .env_remove("CARGO_TARGET_DIR")
                .output()
                .expect("run sandbox processing payload negative");
        assert!(!output.status.success(), "{state} final payload passed");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("make build-sandbox-processing"));
        assert!(stderr.contains(EMPTY_SHA256));
        assert!(
            !argv_log.exists(),
            "check invoked Cargo for {state} payload"
        );
        assert!(
            !acquire_log.exists(),
            "check attempted acquisition/repair for {state} payload"
        );
        assert!(
            !helper_env_log.exists(),
            "check invoked helpers before rejecting {state} payload"
        );
        match state {
            "missing" => {
                assert!(
                    stderr.contains(
                        payload_dir
                            .join(SANDBOX_RUNTIME_NAMES[0])
                            .to_string_lossy()
                            .as_ref()
                    )
                );
                assert!(!payload_dir.exists(), "missing payload was repaired");
            }
            "corrupt" => {
                assert!(
                    stderr.contains(
                        payload_dir
                            .join(SANDBOX_RUNTIME_NAMES[1])
                            .to_string_lossy()
                            .as_ref()
                    )
                );
                assert_eq!(
                    fs::read(payload_dir.join(SANDBOX_RUNTIME_NAMES[1]))
                        .expect("read corrupt final runtime entry"),
                    b"corrupt"
                );
            }
            "symlink" => {
                let linked_runtime = payload_dir.join(SANDBOX_RUNTIME_NAMES[1]);
                assert!(stderr.contains(linked_runtime.to_string_lossy().as_ref()));
                assert_eq!(
                    fs::read(&linked_runtime).expect("read linked runtime content"),
                    b""
                );
                assert!(
                    fs::symlink_metadata(&linked_runtime)
                        .expect("inspect linked final runtime entry")
                        .file_type()
                        .is_symlink()
                );
            }
            _ => unreachable!("fixture state is known"),
        }
    }
}

#[test]
fn sandbox_processing_check_uses_only_the_existing_payload_and_clears_loader_paths() {
    let temp = TempDir::new("sandbox-processing-check");
    write_host_makefile(&temp.path, "Linux", "x86_64");
    let shims = temp.path.join("shims");
    fs::create_dir(&shims).expect("create Cargo shim directory");
    write_acquire_and_build_cargo_shim(&shims.join("cargo"), "");
    let target_dir = temp.path.join("core/target");
    seed_sandbox_payload(&target_dir);
    write_sandbox_processing_helpers(&target_dir);
    let acquire_log = temp.path.join("acquire.log");
    let argv_log = temp.path.join("cargo.argv");
    let helper_env_log = temp.path.join("helper.env");
    let output = fixture_make_command(&temp.path, &shims, "check-rust-sandbox-processing-build")
        .env("SOLSTONE_ACQUIRE_LOG", &acquire_log)
        .env("SOLSTONE_CARGO_ARGV", &argv_log)
        .env("SOLSTONE_CARGO_ENV", temp.path.join("cargo.env"))
        .env("SOLSTONE_HELPER_ENV_LOG", &helper_env_log)
        .env("LD_LIBRARY_PATH", "fixture-loader-path")
        .env("DYLD_LIBRARY_PATH", "fixture-loader-path")
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("run sandbox processing check fixture");
    assert!(
        output.status.success(),
        "sandbox processing check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!argv_log.exists(), "check invoked Cargo");
    assert!(!acquire_log.exists(), "check attempted acquisition/repair");
    let helper_env = fs::read_to_string(&helper_env_log).expect("read helper environment log");
    assert!(helper_env.contains("solstone-core-speakers-analyze LD=unset DYLD=unset"));
    assert!(helper_env.contains("solstone-core-vad-analyze LD=unset DYLD=unset"));
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

    for state in ["missing", "corrupt", "valid"] {
        let temp = TempDir::new(&format!("pdf-readiness-{state}"));
        write_host_makefile(&temp.path, "Linux", "x86_64");
        let library = temp
            .path
            .join("target/pdfium-runtime-link/linux-x86_64/libpdfium.so");
        if state != "missing" {
            fs::create_dir_all(library.parent().expect("PDF runtime parent"))
                .expect("create PDF runtime directory");
            let bytes = if state == "valid" {
                b"".as_slice()
            } else {
                b"corrupt".as_slice()
            };
            fs::write(&library, bytes).expect("write PDF runtime fixture");
        }
        let output = Command::new("make")
            .arg("check-rust-pdf-ready")
            .current_dir(&temp.path)
            .output()
            .expect("run PDF readiness control");
        if state == "valid" {
            assert!(
                output.status.success(),
                "valid PDF runtime failed readiness: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            assert!(
                !output.status.success(),
                "{state} PDF runtime passed readiness"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("invalid pinned host PDFium runtime file"));
            assert!(stderr.contains("run 'make check-rust-pdf-stage' and retry"));
        }
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
    let acquire_sentinel = temp.path.join("acquire-ran");
    write_acquire_shim(
        &shims.join("cargo"),
        "    : > \"$SOLSTONE_ACQUIRE_SENTINEL\"\n    exit 97\n",
    );
    let output = Command::new("make")
        .arg("check-rust-onnx-stage")
        .current_dir(&temp.path)
        .env("PATH", fixture_path(&shims))
        .env("SOLSTONE_HASH_LOG", &hash_log)
        .env("SOLSTONE_ACQUIRE_SENTINEL", &acquire_sentinel)
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
        acquire_sentinel.exists(),
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
fn service_legacy_evidence_command_graph_is_selectable_and_resolves_staging_root() {
    let root = repo_root();
    let root_metadata_output = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            "core/Cargo.toml",
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
        ])
        .current_dir(&root)
        .output()
        .expect("root cargo metadata runs");
    assert!(
        root_metadata_output.status.success(),
        "root cargo metadata must succeed: {}",
        String::from_utf8_lossy(&root_metadata_output.stderr)
    );
    let root_metadata: serde_json::Value =
        serde_json::from_slice(&root_metadata_output.stdout).expect("root metadata JSON parses");
    let selected_ids = root_metadata["workspace_members"]
        .as_array()
        .expect("workspace_members array");
    let default_ids = root_metadata["workspace_default_members"]
        .as_array()
        .expect("workspace_default_members array");
    for ids in [selected_ids, default_ids] {
        assert!(
            ids.iter().all(|id| !id
                .as_str()
                .expect("package id string")
                .contains("solstone-core-service-legacy-evidence")),
            "ordinary root Cargo selection must exclude the evidence package"
        );
    }

    let standalone = root.join("core/crates/solstone-core-service-legacy-evidence/Cargo.toml");
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            standalone.to_str().expect("manifest path is UTF-8"),
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
        ])
        .current_dir(&root)
        .output()
        .expect("standalone cargo metadata runs");
    assert!(
        output.status.success(),
        "standalone evidence manifest must remain directly selectable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("metadata JSON parses");
    let packages = metadata["packages"].as_array().expect("packages array");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"], "solstone-core-service-legacy-evidence");

    for target in [
        "check-service-legacy-evidence",
        "service-legacy-evidence-capture",
    ] {
        let dry_run = Command::new("make")
            .args(["-n", target])
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&root)
            .output()
            .expect("uv-free make dry run starts");
        assert!(
            dry_run.status.success(),
            "{target} must be selectable without uv: {}",
            String::from_utf8_lossy(&dry_run.stderr)
        );
    }

    let staged = "/tmp/service-legacy-staged-evidence-control";
    let make_fragment = tempfile::NamedTempFile::new().expect("temporary Make fragment opens");
    fs::write(
        make_fragment.path(),
        "print-service-evidence-root:\n\t@printf '%s\\n' '$(SERVICE_LEGACY_EVIDENCE_ROOT)'\n",
    )
    .expect("temporary Make fragment writes");
    let resolved = Command::new("make")
        .args(["-s", "--no-print-directory", "-f", "Makefile", "-f"])
        .arg(make_fragment.path())
        .args(["print-service-evidence-root", "UV=/bin/true"])
        .env("SERVICE_LEGACY_EVIDENCE_ROOT", staged)
        .current_dir(&root)
        .output()
        .expect("make resolves the staged evidence root");
    assert!(
        resolved.status.success(),
        "make failed to resolve staged evidence root: {}",
        String::from_utf8_lossy(&resolved.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&resolved.stdout).trim(), staged);
}

fn windows_transport_fixture(name: &str) -> TempDir {
    let temp = TempDir::new(name);
    let scripts = temp.path.join("scripts");
    fs::create_dir(&scripts).expect("create transport fixture scripts");
    write_executable(
        &scripts.join("check-win-sync-tree.sh"),
        include_str!("../../../../scripts/check-win-sync-tree.sh"),
    );
    write_executable(
        &scripts.join("sync-win-host.sh"),
        include_str!("../../../../scripts/sync-win-host.sh"),
    );
    write_executable(
        &scripts.join("win-host-ci.sh"),
        include_str!("../../../../scripts/win-host-ci.sh"),
    );
    fs::create_dir(temp.path.join("core")).expect("create fixture core directory");
    fs::write(temp.path.join("core/Cargo.lock"), b"version = 4\n")
        .expect("write fixture Cargo.lock");
    for arguments in [
        &["init", "-q"][..],
        &["config", "user.name", "transport fixture"][..],
        &["config", "user.email", "transport@example.invalid"][..],
        &[
            "add",
            "core/Cargo.lock",
            "scripts/check-win-sync-tree.sh",
            "scripts/sync-win-host.sh",
            "scripts/win-host-ci.sh",
        ][..],
        &["commit", "-q", "-m", "fixture"][..],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&temp.path)
            .output()
            .expect("run fixture Git command");
        assert!(
            output.status.success(),
            "fixture git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    temp
}

fn write_transport_scp_shim(temp: &TempDir) -> (PathBuf, PathBuf) {
    let shim = temp.path.join(".git/scp-shim");
    let log = temp.path.join(".git/scp.log");
    write_executable(
        &shim,
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$SOLSTONE_SCP_LOG\"\n",
    );
    (shim, log)
}

fn write_transport_ssh_shim(temp: &TempDir) -> (PathBuf, PathBuf) {
    let shim = temp.path.join(".git/ssh-shim");
    let log = temp.path.join(".git/ssh.log");
    write_executable(
        &shim,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SOLSTONE_SSH_LOG"
snapshot_sha=$(sed -n 's/^  "commit": "\([0-9a-f]*\)",$/\1/p' target/win-host-ci-source-binding.json)
cargo_lock_sha256=$(sed -n 's/^  "cargo_lock_sha256": "\([0-9a-f]*\)"$/\1/p' target/win-host-ci-source-binding.json)
case "$*" in
  *"\$env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST = '1'"*) expected=passed ;;
  *"\$env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST = '0'"*) expected=skipped ;;
  *) expected=missing-forward ;;
esac
case "$*" in
  *"\$env:SOLSTONE_JOURNAL_WIN_REFS_ROOT = 'C:\refs'"*)
    refs_requested=1
    refs_enumeration_evidence=executed/pass
    refs_enumeration_capability=available
    refs_revalidation_evidence=executed/pass
    refs_revalidation_capability=available
    refs_archive_traversal_evidence=executed/pass
    refs_archive_traversal_capability=available
    ;;
  *)
    refs_requested=0
    refs_enumeration_evidence=unrun/skipped
    refs_enumeration_capability=not-asserted
    refs_revalidation_evidence=unrun/skipped
    refs_revalidation_capability=not-asserted
    refs_archive_traversal_evidence=unrun/skipped
    refs_archive_traversal_capability=not-asserted
    ;;
esac
refs_claimed_removal_evidence=unrun/skipped
refs_claimed_removal_capability=unsupported
printf 'JOURNAL_WIN_CI_HEAD=%s\n' "$snapshot_sha"
printf 'JOURNAL_WIN_CI_CARGO_LOCK_SHA256=%s\n' "$cargo_lock_sha256"
case "${SOLSTONE_SSH_SCENARIO:-valid}" in
  valid) printf 'JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%s\n' "$expected" ;;
  missing) ;;
  duplicate)
    printf 'JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%s\n' "$expected"
    printf 'JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%s\n' "$expected"
    ;;
  wrong)
    if [ "$expected" = passed ]; then wrong=skipped; else wrong=passed; fi
    printf 'JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%s\n' "$wrong"
    ;;
  unknown) printf 'JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=unknown\n' ;;
  prefixed) printf 'prefix-JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%s\n' "$expected" ;;
  suffixed) printf 'JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%s-suffix\n' "$expected" ;;
  post) printf '%s\n' '=== JOURNAL_WIN_CI_OK: fixture ===' ;;
  *) exit 97 ;;
esac
if [ "${SOLSTONE_SSH_SCENARIO:-valid}" != post ]; then
  printf '%s\n' 'JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed'
  if [ "$refs_requested" -eq 1 ]; then
    printf 'JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=%s\n' "$refs_enumeration_evidence"
    printf 'JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=%s\n' "$refs_enumeration_capability"
    printf 'JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=%s\n' "$refs_revalidation_evidence"
    printf 'JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=%s\n' "$refs_revalidation_capability"
    printf 'JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=%s\n' "$refs_claimed_removal_evidence"
    printf 'JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=%s\n' "$refs_claimed_removal_capability"
    printf 'JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=%s\n' "$refs_archive_traversal_evidence"
    printf 'JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=%s\n' "$refs_archive_traversal_capability"
  fi
  printf '%s\n' '=== JOURNAL_WIN_CI_OK: fixture ==='
else
  printf 'JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%s\n' "$expected"
  printf '%s\n' 'JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed'
  if [ "$refs_requested" -eq 1 ]; then
    printf 'JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=%s\n' "$refs_enumeration_evidence"
    printf 'JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=%s\n' "$refs_enumeration_capability"
    printf 'JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=%s\n' "$refs_revalidation_evidence"
    printf 'JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=%s\n' "$refs_revalidation_capability"
    printf 'JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=%s\n' "$refs_claimed_removal_evidence"
    printf 'JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=%s\n' "$refs_claimed_removal_capability"
    printf 'JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=%s\n' "$refs_archive_traversal_evidence"
    printf 'JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=%s\n' "$refs_archive_traversal_capability"
  fi
fi
"#,
    );
    (shim, log)
}

fn run_windows_driver(
    temp: &TempDir,
    scp: &Path,
    scp_log: &Path,
    ssh: &Path,
    ssh_log: &Path,
    opt_in: Option<&str>,
    scenario: &str,
) -> std::process::Output {
    run_windows_driver_with_refs(
        temp,
        WindowsTransport {
            scp,
            scp_log,
            ssh,
            ssh_log,
        },
        opt_in,
        None,
        scenario,
    )
}

struct WindowsTransport<'a> {
    scp: &'a Path,
    scp_log: &'a Path,
    ssh: &'a Path,
    ssh_log: &'a Path,
}

fn run_windows_driver_with_refs(
    temp: &TempDir,
    transport: WindowsTransport<'_>,
    opt_in: Option<&str>,
    refs_root: Option<&str>,
    scenario: &str,
) -> std::process::Output {
    let mut command = Command::new("sh");
    command
        .arg("scripts/win-host-ci.sh")
        .current_dir(&temp.path)
        .env("WIN_REMOTE_HOST", "fake@example.invalid")
        .env("SCP", transport.scp)
        .env("SSH", transport.ssh)
        .env("SOLSTONE_SCP_LOG", transport.scp_log)
        .env("SOLSTONE_SSH_LOG", transport.ssh_log)
        .env("SOLSTONE_SSH_SCENARIO", scenario)
        .env("SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT", "solbuild");
    if let Some(value) = opt_in {
        command.env("JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST", value);
    } else {
        command.env_remove("JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST");
    }
    if let Some(value) = refs_root {
        command.env("SOLSTONE_JOURNAL_WIN_REFS_ROOT", value);
    } else {
        command.env_remove("SOLSTONE_JOURNAL_WIN_REFS_ROOT");
    }
    command.output().expect("run Windows native-gate driver")
}

#[test]
fn windows_native_sync_refuses_untracked_inputs_before_transfer() {
    let temp = windows_transport_fixture("windows-native-sync-untracked");
    let (scp, scp_log) = write_transport_scp_shim(&temp);
    fs::write(
        temp.path.join("untracked.txt"),
        b"must not disappear from bundle",
    )
    .expect("write untracked fixture");

    let output = Command::new("sh")
        .arg("scripts/sync-win-host.sh")
        .current_dir(&temp.path)
        .env("WIN_REMOTE_HOST", "fake@example.invalid")
        .env("SCP", &scp)
        .env("SOLSTONE_SCP_LOG", &scp_log)
        .output()
        .expect("run untracked-input Windows sync fixture");
    assert!(
        !output.status.success(),
        "untracked input passed Windows sync"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("untracked non-ignored files would be omitted"));
    assert!(stderr.contains("untracked.txt"));
    assert!(!scp_log.exists(), "refused sync invoked scp");
    assert!(
        !temp
            .path
            .join("target/win-host-ci-source-binding.json")
            .exists(),
        "refused sync left an authoritative-looking binding"
    );
}

#[test]
fn windows_native_sync_binds_and_transfers_the_exact_dirty_tree() {
    let temp = windows_transport_fixture("windows-native-sync-dirty-tree");
    let (scp, scp_log) = write_transport_scp_shim(&temp);
    let dirty_lock = b"version = 4\n# dirty snapshot control\n";
    fs::write(temp.path.join("core/Cargo.lock"), dirty_lock).expect("dirty fixture Cargo.lock");

    let output = Command::new("sh")
        .arg("scripts/sync-win-host.sh")
        .current_dir(&temp.path)
        .env("WIN_REMOTE_HOST", "fake@example.invalid")
        .env("SCP", &scp)
        .env("SOLSTONE_SCP_LOG", &scp_log)
        .output()
        .expect("run dirty-tree Windows sync fixture");
    assert!(
        output.status.success(),
        "dirty tracked tree failed Windows sync:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("SYNC_WIN_HOST_OK"));

    let binding_path = temp.path.join("target/win-host-ci-source-binding.json");
    let binding: serde_json::Value =
        serde_json::from_slice(&fs::read(&binding_path).expect("read Windows source binding"))
            .expect("parse Windows source binding");
    assert_eq!(binding["schema"], "solstone.journal.win-source-binding.v1");
    let snapshot = binding["commit"].as_str().expect("binding commit");
    assert_eq!(snapshot.len(), 40);
    let bundled_lock = Command::new("git")
        .args(["show", &format!("{snapshot}:core/Cargo.lock")])
        .current_dir(&temp.path)
        .output()
        .expect("read bundled Cargo.lock");
    assert!(bundled_lock.status.success());
    assert_eq!(bundled_lock.stdout, dirty_lock);

    let scp_calls = fs::read_to_string(&scp_log).expect("read scp calls");
    assert_eq!(
        scp_calls.lines().count(),
        2,
        "unexpected scp calls: {scp_calls}"
    );
    assert!(scp_calls.contains("fake@example.invalid:sjbuild.bundle"));
    assert!(scp_calls.contains("fake@example.invalid:journal-win-host-ci-source-binding.json"));
    let ref_probe = Command::new("git")
        .args(["show-ref", "--verify", "refs/heads/__sjwsync"])
        .current_dir(&temp.path)
        .output()
        .expect("probe temporary sync ref");
    assert!(
        !ref_probe.status.success(),
        "successful sync left refs/heads/__sjwsync behind"
    );
}

#[test]
fn windows_native_driver_rejects_invalid_cloud_opt_in_before_transport() {
    for (index, invalid) in [
        "2",
        "01",
        "true",
        " ",
        "1&echo injected",
        "$(echo injected)",
    ]
    .into_iter()
    .enumerate()
    {
        let temp = windows_transport_fixture(&format!("windows-native-driver-invalid-{index}"));
        let sync_log = temp.path.join(".git/sync.log");
        write_executable(
            &temp.path.join("scripts/sync-win-host.sh"),
            "#!/bin/sh\nprintf 'invoked\\n' > .git/sync.log\nexit 99\n",
        );
        let (scp, scp_log) = write_transport_scp_shim(&temp);
        let (ssh, ssh_log) = write_transport_ssh_shim(&temp);
        let output = run_windows_driver(
            &temp,
            &scp,
            &scp_log,
            &ssh,
            &ssh_log,
            Some(invalid),
            "valid",
        );
        assert!(
            !output.status.success(),
            "invalid opt-in {invalid:?} passed"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("must be unset, empty, 0, or 1"),
            "invalid opt-in {invalid:?} did not reach the input boundary: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!sync_log.exists(), "invalid opt-in invoked sync");
        assert!(!scp_log.exists(), "invalid opt-in invoked scp");
        assert!(!ssh_log.exists(), "invalid opt-in invoked ssh");
    }
}

#[test]
fn windows_native_driver_forwards_only_normalized_cloud_opt_in() {
    for (index, opt_in, expected) in [
        (0, None, "0"),
        (1, Some(""), "0"),
        (2, Some("0"), "0"),
        (3, Some("1"), "1"),
    ] {
        let temp = windows_transport_fixture(&format!("windows-native-driver-forward-{index}"));
        let (scp, scp_log) = write_transport_scp_shim(&temp);
        let (ssh, ssh_log) = write_transport_ssh_shim(&temp);
        let output = run_windows_driver(&temp, &scp, &scp_log, &ssh, &ssh_log, opt_in, "valid");
        assert!(
            output.status.success(),
            "normalized opt-in {opt_in:?} failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let ssh_call = fs::read_to_string(&ssh_log).expect("read ssh command");
        let forwarded = format!("$env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST = '{expected}'");
        assert_eq!(
            ssh_call
                .matches("$env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST = '")
                .count(),
            1,
            "opt-in {opt_in:?} produced an ambiguous remote assignment"
        );
        assert!(ssh_call.contains("$env:SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT = 'solbuild'"));
        assert_eq!(
            ssh_call.matches(&forwarded).count(),
            1,
            "opt-in {opt_in:?} was not forwarded as one literal {expected} byte"
        );
        let evidence = if expected == "1" { "passed" } else { "skipped" };
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("JOURNAL_WIN_HOST_CI_VERIFIED commit=")
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains(&format!("cloud_sync_evidence={evidence}"))
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("ordinary_owner_evidence=passed"));
    }
}

#[test]
fn windows_native_driver_forwards_or_skips_the_refs_matrix_fixture() {
    let temp = windows_transport_fixture("windows-native-driver-refs");
    let (scp, scp_log) = write_transport_scp_shim(&temp);
    let (ssh, ssh_log) = write_transport_ssh_shim(&temp);
    let output = run_windows_driver_with_refs(
        &temp,
        WindowsTransport {
            scp: &scp,
            scp_log: &scp_log,
            ssh: &ssh,
            ssh_log: &ssh_log,
        },
        None,
        Some("C:\\refs"),
        "valid",
    );
    assert!(
        output.status.success(),
        "valid ReFS fixture failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ssh_call = fs::read_to_string(&ssh_log).expect("read ssh command");
    assert!(ssh_call.contains("$env:SOLSTONE_JOURNAL_WIN_REFS_ROOT = 'C:\\refs'"));
    let receipt = String::from_utf8_lossy(&output.stdout);
    assert!(receipt.contains("refs_enumeration_evidence=executed/pass"));
    assert!(receipt.contains("refs_enumeration_capability=available"));
    assert!(receipt.contains("refs_revalidation_evidence=executed/pass"));
    assert!(receipt.contains("refs_revalidation_capability=available"));
    assert!(receipt.contains("refs_claimed_removal_evidence=unrun/skipped"));
    assert!(receipt.contains("refs_claimed_removal_capability=unsupported"));
    assert!(receipt.contains("refs_archive_traversal_evidence=executed/pass"));
    assert!(receipt.contains("refs_archive_traversal_capability=available"));

    let invalid = run_windows_driver_with_refs(
        &temp,
        WindowsTransport {
            scp: &scp,
            scp_log: &scp_log,
            ssh: &ssh,
            ssh_log: &ssh_log,
        },
        None,
        Some("not-a-windows-path"),
        "valid",
    );
    assert!(invalid.status.success(), "invalid fixture must be skipped");
    let invalid_receipt = String::from_utf8_lossy(&invalid.stdout);
    assert!(invalid_receipt.contains("refs_enumeration_evidence=unrun/skipped"));
    assert!(invalid_receipt.contains("refs_enumeration_capability=not-asserted"));
}

#[test]
fn windows_native_driver_rejects_ambiguous_cloud_evidence() {
    for scenario in [
        "missing",
        "duplicate",
        "wrong",
        "unknown",
        "prefixed",
        "suffixed",
        "post",
    ] {
        let temp = windows_transport_fixture(&format!("windows-native-driver-{scenario}"));
        let (scp, scp_log) = write_transport_scp_shim(&temp);
        let (ssh, ssh_log) = write_transport_ssh_shim(&temp);
        let output = run_windows_driver(&temp, &scp, &scp_log, &ssh, &ssh_log, Some("1"), scenario);
        assert!(
            !output.status.success(),
            "ambiguous evidence scenario {scenario} passed:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

fn validate_windows_runner_contract(runner: &str, limited_child: &str) -> Result<(), String> {
    let cloud_integration = "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-io --test windows_cloud_sync_root_registration --features test-hooks || exit /b 1";
    let cloud_passed = "set \"JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=passed\"";
    let cloud_evidence =
        "echo JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=%JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE%";
    let ordinary_integration = "pub const ORDINARY_OWNER_CARGO_TEST: &str = \"cargo test --manifest-path core\\\\Cargo.toml --locked -p solstone-core-journal-io --test windows_ordinary_owner_inventory --features test-hooks -- --nocapture\";";
    let ordinary_marker = "[regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed\\r?$').Count -eq 1";
    let ordinary_refs_marker = "[regex]::Matches($text, '(?m)^JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed\\r?$').Count -eq 1";
    let ordinary_status = "set \"JOURNAL_WIN_CI_ORDINARY_OWNER_STATUS=%ERRORLEVEL%\"";
    let ordinary_passed = "set \"JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed\"";
    let ordinary_evidence =
        "echo JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=%JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE%";
    let refs_markers = [
        "echo JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=%JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE%",
        "echo JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=%JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY%",
        "echo JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=%JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE%",
        "echo JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=%JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY%",
        "echo JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=%JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE%",
        "echo JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=%JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY%",
        "echo JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=%JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE%",
        "echo JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=%JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY%",
    ];
    let ok_prefix = "echo === JOURNAL_WIN_CI_OK:";
    let lines = runner.lines().map(str::trim).collect::<Vec<_>>();

    let unique_position = |needle: &str| -> Result<usize, String> {
        let positions = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| (*line == needle).then_some(index))
            .collect::<Vec<_>>();
        if positions.len() == 1 {
            Ok(positions[0])
        } else {
            Err(format!(
                "expected one {needle:?}, found {}",
                positions.len()
            ))
        }
    };

    if !limited_child.contains(ordinary_integration)
        || !runner.contains(ordinary_marker)
        || !runner.contains(ordinary_refs_marker)
        || !runner.contains(ordinary_status)
        || !runner.contains("recover-held --lease")
        || !runner.contains("prepare --lease")
        || !runner.contains("launch --lease")
        || !runner.contains("await --lease")
        || !runner.contains("cleanup --lease")
    {
        return Err("ordinary-owner rail must retain its limited-child command, lifecycle, exit status, and terminal marker check".to_owned());
    }

    let cloud_integration_position = unique_position(cloud_integration)?;
    let cloud_passed_position = unique_position(cloud_passed)?;
    let cloud_evidence_position = unique_position(cloud_evidence)?;
    let ordinary_passed_position = unique_position(ordinary_passed)?;
    let ordinary_evidence_position = unique_position(ordinary_evidence)?;
    let refs_positions = refs_markers
        .into_iter()
        .map(unique_position)
        .collect::<Result<Vec<_>, _>>()?;
    let ok_positions = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with(ok_prefix).then_some(index))
        .collect::<Vec<_>>();
    if ok_positions.len() != 1 {
        return Err(format!(
            "expected one JOURNAL_WIN_CI_OK marker, found {}",
            ok_positions.len()
        ));
    }
    if cloud_integration_position >= cloud_passed_position {
        return Err("passed evidence precedes its gated integration command".to_owned());
    }
    if ordinary_passed_position >= ordinary_evidence_position {
        return Err(
            "ordinary-owner evidence is echoed before its successful assignment".to_owned(),
        );
    }
    if cloud_passed_position >= cloud_evidence_position
        || cloud_evidence_position >= ordinary_evidence_position
        || ordinary_evidence_position >= refs_positions[0]
        || refs_positions
            .iter()
            .any(|position| *position >= ok_positions[0])
    {
        return Err("evidence assignment/echo/OK ordering is invalid".to_owned());
    }
    Ok(())
}

#[test]
fn windows_native_runner_evidence_validator_rejects_false_green_mutations() {
    let runner = include_str!("../../../../scripts/win-ci.cmd");
    let limited_child = include_str!("../../../crates/solstone-core-win-owner-rail/src/windows.rs");
    validate_windows_runner_contract(runner, limited_child)
        .expect("live Windows runner evidence contract");

    let cloud_integration = "  cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-io --test windows_cloud_sync_root_registration --features test-hooks || exit /b 1";
    let cloud_passed = "  set \"JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=passed\"";
    let ordinary_integration = "pub const ORDINARY_OWNER_CARGO_TEST: &str = \"cargo test --manifest-path core\\\\Cargo.toml --locked -p solstone-core-journal-io --test windows_ordinary_owner_inventory --features test-hooks -- --nocapture\";";
    let ordinary_evidence =
        "echo JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=%JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE%";
    let refs_archive_capability = "echo JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=%JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY%";
    let ok = runner
        .lines()
        .find(|line| line.starts_with("echo === JOURNAL_WIN_CI_OK:"))
        .expect("live OK marker");
    let mutations = [
        (
            "early passed assignment",
            runner.replacen(
                &format!("{cloud_integration}\n{cloud_passed}"),
                &format!("{cloud_passed}\n{cloud_integration}"),
                1,
            ),
        ),
        (
            "duplicate ordinary-owner evidence echo",
            runner.replacen(
                ordinary_evidence,
                &format!("{ordinary_evidence}\n{ordinary_evidence}"),
                1,
            ),
        ),
        (
            "skipped ordinary-owner limited-child command",
            limited_child.replacen(ordinary_integration, "", 1),
        ),
        (
            "post-OK evidence",
            runner.replacen(ok, &format!("{ok}\n{refs_archive_capability}"), 1),
        ),
    ];
    for (name, mutation) in mutations {
        let (mutated_runner, mutated_child) =
            if name == "skipped ordinary-owner limited-child command" {
                (runner.to_owned(), mutation)
            } else {
                (mutation, limited_child.to_owned())
            };
        assert!(
            mutated_runner.as_str() != runner || mutated_child.as_str() != limited_child,
            "mutation fixture {name} changed nothing"
        );
        assert!(
            validate_windows_runner_contract(&mutated_runner, &mutated_child).is_err(),
            "runner validator accepted {name}"
        );
    }
}

#[test]
fn windows_native_gate_is_isolated_and_names_its_evidence_boundary() {
    let root = repo_root();
    let makefile = fs::read_to_string(root.join("Makefile")).expect("read Makefile");
    let sync = fs::read_to_string(root.join("scripts/sync-win-host.sh"))
        .expect("read Windows sync script");
    let driver = fs::read_to_string(root.join("scripts/win-host-ci.sh"))
        .expect("read Windows driver script");
    let runner = fs::read_to_string(root.join("scripts/win-ci.cmd")).expect("read Windows runner");
    let limited_child =
        fs::read_to_string(root.join("core/crates/solstone-core-win-owner-rail/src/windows.rs"))
            .expect("read Windows limited child");

    assert!(makefile.contains("win-host-ci: require-win-remote-host"));
    assert!(makefile.contains("sh scripts/win-host-ci.sh"));
    for token in [
        "refs/heads/__sjwsync",
        "sjbuild.bundle",
        "journal-win-host-ci-source-binding.json",
        "ControlPath=/tmp/sj-%r@%h:%p",
    ] {
        assert!(
            sync.contains(token) || driver.contains(token),
            "Windows transport lost isolated token {token}"
        );
    }
    assert!(driver.contains("solstone-journal-win-host-ci.lock"));
    assert!(driver.contains("C:\\\\sol\\\\sj-ci.cmd"));
    assert!(runner.contains(
        "cargo build --manifest-path core\\Cargo.toml --locked -p solstone-core-journal -p solstone-core-journal-config -p solstone-core-journal-io -p solstone-core-win-owner-rail"
    ));
    assert!(runner.contains(
        "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-config --lib"
    ));
    for command in [
        "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-io --lib",
        "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-io --test journal_io_lock_component --features test-hooks",
        "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-io --test windows_cloud_sync_root_registration --features test-hooks",
        "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-archive --lib source_freezes_portable_members_and_checked_bytes -- --nocapture",
        "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal --lib",
    ] {
        assert!(runner.contains(command), "Windows runner missing {command}");
    }
    assert!(limited_child.contains("pub const ORDINARY_OWNER_CARGO_TEST: &str = \"cargo test --manifest-path core\\\\Cargo.toml --locked -p solstone-core-journal-io --test windows_ordinary_owner_inventory --features test-hooks -- --nocapture\";"));
    assert!(runner.contains("if \"%JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST%\"==\"1\""));
    assert!(
        runner
            .contains("if ($env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST -notmatch '^[01]$') { exit 1 }")
    );
    assert!(runner.contains("set \"JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=skipped\""));
    assert!(runner.contains("set \"JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=passed\""));
    for token in [
        "SOLSTONE_JOURNAL_WIN_REFS_ROOT",
        "SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT",
        "solstone-core-win-owner-rail.exe",
        "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE",
        "JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed",
        "JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE",
        "JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY",
        "JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE",
        "JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY",
        "JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE",
        "JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY",
        "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE",
        "JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY",
        "source_freezes_portable_members_and_checked_bytes",
    ] {
        assert!(runner.contains(token), "Windows runner missing {token}");
    }
    assert!(runner.contains(
        "echo === cargo test --locked journal-io Cloud Files sync-root registration ==="
    ));
    assert!(!runner.contains(
        "echo === cargo test --locked (journal-io Cloud Files sync-root registration) ==="
    ));
    assert!(driver.contains("$env:SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT = '$owner_account'"));
    validate_windows_runner_contract(&runner, &limited_child)
        .expect("Windows runner evidence contract");
    for test in [
        "config_strip_matches_python_control_whitespace",
        "ensure_journal_dir_reports_non_directory_parent",
    ] {
        assert!(
            runner.contains(&format!("findstr /c:\"tests::{test}: test\"")),
            "Windows runner must list required test {test}"
        );
        assert!(
            runner.contains(&format!("call :require_journal_test tests::{test}")),
            "Windows runner must require execution of {test}"
        );
        assert!(
            runner.contains(&format!("ERROR: required journal test {test}")),
            "Windows runner must name a failed required test {test}"
        );
    }
    assert_eq!(
        runner
            .matches("call :verify_source_binding || exit /b 1")
            .count(),
        2,
        "Windows runner must verify the exact source both before and after native work"
    );
    assert!(runner.contains(
        "JOURNAL_WIN_CI_OK: native Windows MSVC build passed for solstone-core-journal-io solstone-core-journal and solstone-core-journal-config; journal-io library and lock-component tests and journal library tests including config_strip_matches_python_control_whitespace and ensure_journal_dir_reports_non_directory_parent passed; ordinary-owner inventory evidence %JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE%; Cloud Files sync-root registration evidence %JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE%; ReFS enumeration evidence %JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY%; ReFS revalidation evidence %JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY%; ReFS claimed-removal evidence %JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY%; ReFS archive traversal evidence %JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE% capability %JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY%; publication locking beyond the named lock component Callosum packaging install signing and smoke not run"
    ));
    assert!(!sync.contains("swbuild.bundle"));
    assert!(!driver.contains("C:\\\\sol\\\\sw-ci.cmd"));
}
