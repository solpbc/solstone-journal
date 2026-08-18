// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

include!("support/repository_make_command_graphs.rs");

#[test]
fn make_ci_never_executes_forbidden_interpreters() {
    assert_gate_never_executes_forbidden_interpreters("ci", &["fmt", "run", "clippy", "test"]);
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
