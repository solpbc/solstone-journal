// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_repository_contracts::ci::{Registry, load_registry};

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
    let script = "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> \"$SOLSTONE_CI_CARGO_LOG\"\nif [ -n \"${SOLSTONE_CI_CARGO_ENV_LOG:-}\" ]; then\n    printf '%s|%s|%s\\n' \"${CARGO_INCREMENTAL-unset}\" \"${CARGO_PROFILE_DEV_DEBUG-unset}\" \"${CARGO_TARGET_DIR-unset}\" >> \"$SOLSTONE_CI_CARGO_ENV_LOG\"\nfi\ncase \"$*\" in\n  *'--bin solstone-core-depict'*)\n    printf '%s\\n' '{\"schema\":\"solstone-depict-error-v1\",\"reason\":\"malformed-request\"}' >&2\n    exit 1\n    ;;\n  *'--bin solstone-core-speakers-analyze'*)\n    printf '%s\\n' '{\"schema\":\"solstone-speaker-analyze-error-v1\",\"reason\":\"malformed-request\"}' >&2\n    exit 64\n    ;;\n  *'--bin solstone-core-vad-analyze'*)\n    printf '%s\\n' '{\"schema\":\"solstone-vad-error-v1\",\"reason\":\"malformed-request\"}' >&2\n    exit 64\n    ;;\nesac\n";
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
    FILE *args = fopen(getenv("SOLSTONE_CARGO_ARGV"), "ab");
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
        "#!/bin/sh\nset -eu\nprintf '%s\\0' \"$@\" >> \"$SOLSTONE_CARGO_ARGV\"\nprintf 'ORT_PREFER_DYNAMIC_LINK=%s\\nORT_LIB_PATH=%s\\nDYLD_LIBRARY_PATH=%s\\nLD_LIBRARY_PATH=%s\\n' \"${ORT_PREFER_DYNAMIC_LINK-}\" \"${ORT_LIB_PATH-}\" \"${DYLD_LIBRARY_PATH-}\" \"${LD_LIBRARY_PATH-}\" > \"$SOLSTONE_CARGO_ENV\"\n",
    );
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
        replace_once(
            &mut makefile,
            &format!("override PDF_RUNTIME_{digest_key}_DIGEST := "),
            &format!("override PDF_RUNTIME_{digest_key}_DIGEST := {EMPTY_SHA256}#"),
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
    let live_use_script = root.join("scripts/check_rust_target_live_use.sh");
    fs::create_dir_all(live_use_script.parent().expect("live-use script parent"))
        .expect("create live-use script parent");
    write_executable(
        &live_use_script,
        include_str!("../../../../../scripts/check_rust_target_live_use.sh"),
    );
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

fn write_acquire_shim(path: &Path, body: &str) {
    write_executable(
        path,
        &format!(
            "#!/bin/sh\nset -eu\nif [ -n \"${{SOLSTONE_ACQUIRE_LOG:-}}\" ]; then\nprintf '%s\\0' \"$@\" > \"$SOLSTONE_ACQUIRE_LOG\"\nfi\ncase \" $* \" in\n  *\" acquire \"*)\n{body}\n    ;;\nesac\nexit 97\n"
        ),
    );
}

fn write_acquire_and_build_cargo_shim(path: &Path, acquire_body: &str) {
    write_executable(
        path,
        &format!(
            "#!/bin/sh\nset -eu\ncase \" $* \" in\n  *\" acquire \"*)\n    if [ -n \"${{SOLSTONE_ACQUIRE_LOG:-}}\" ]; then\n        printf '%s\\0' \"$@\" >> \"$SOLSTONE_ACQUIRE_LOG\"\n    fi\n{acquire_body}\n    exit 0\n    ;;\nesac\nprintf '%s\\0' \"$@\" > \"$SOLSTONE_CARGO_ARGV\"\nprintf 'ORT_PREFER_DYNAMIC_LINK=%s\\nORT_LIB_PATH=%s\\nDYLD_LIBRARY_PATH=%s\\nLD_LIBRARY_PATH=%s\\n' \"${{ORT_PREFER_DYNAMIC_LINK-}}\" \"${{ORT_LIB_PATH-}}\" \"${{DYLD_LIBRARY_PATH-}}\" \"${{LD_LIBRARY_PATH-}}\" > \"$SOLSTONE_CARGO_ENV\"\n"
        ),
    );
}

fn nul_argv(path: &Path) -> Vec<String> {
    fs::read(path)
        .expect("read NUL argv log")
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8(part.to_vec()).expect("argv must be UTF-8"))
        .collect()
}

fn makefile_text(root: &Path) -> String {
    fs::read_to_string(root.join("Makefile")).expect("read Makefile")
}

fn ci_registry(root: &Path) -> Registry {
    load_registry(&root.join("core/ci/suites.toml")).expect("load CI suite registry")
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
    let writable_dir = root.join("core/target");
    let sentinel = writable_dir.join("sentinel.log");
    let cargo_log = writable_dir.join("cargo.log");
    fs::create_dir(&shim_dir).expect("create shim directory");
    fs::create_dir_all(&venv_bin).expect("create poison virtualenv bin directory");
    fs::create_dir_all(&writable_dir).expect("create writable Cargo target fixture directory");
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
        .env("CARGO_TARGET_DIR", &writable_dir)
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
