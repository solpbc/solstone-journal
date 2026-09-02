// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared on-disk CED install used by tests in the consumer crates.
//!
//! One definition of a correct install so `local`, `sound-tags`,
//! `install-models`, `check`, and `health` tests do not each invent a layout.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;

use super::ced_install::{
    CedInstallError, ced_library_path, ced_model_path, engine_dir, library_name, model_artifact,
    write_model_sidecar, write_sidecar,
};
use super::manifest::sha256_file;

const LOAD_STUB: &str = r#"
#include <stdlib.h>
int ced_capi_abi_version(void) { return 1; }
void *ced_capi_load(const char *path) {
    (void)path;
    return malloc(1);
}
void ced_capi_free(void *ctx) { free(ctx); }
const char *ced_capi_last_error(const void *ctx) {
    (void)ctx;
    return "stub failure";
}
char *ced_capi_classify_pcm_json(void *ctx, const float *samples, int n, int rate, int top_k) {
    (void)ctx;
    (void)samples;
    (void)n;
    (void)rate;
    (void)top_k;
    return NULL;
}
void ced_capi_free_string(char *text) { free(text); }
"#;

/// Write a sidecar from [`record`] plus nonempty engine files and a
/// catalog-sized model. Model bytes are zeros — not the catalog digest —
/// until the caller overwrites them.
pub fn write_complete_ced_install(journal: &Path, key: &str) -> Result<(), CedInstallError> {
    write_sidecar(journal, key)?;
    let engine = engine_dir(journal, key);
    fs::create_dir_all(&engine)
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    for name in [library_name(key), "ced_capi.h", "LICENSE", "README.md"] {
        fs::write(engine.join(name), b"nonempty fixture")
            .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    }
    let model = ced_model_path(journal);
    fs::create_dir_all(model.parent().expect("model parent"))
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    let size = model_artifact()?.size_bytes;
    let file = File::create(&model)
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    file.set_len(size)
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    Ok(())
}

/// Overwrite the installed model with `bytes`, then pad to the catalog size.
pub fn write_ced_model_bytes(journal: &Path, bytes: &[u8]) -> Result<(), CedInstallError> {
    let path = ced_model_path(journal);
    fs::create_dir_all(path.parent().expect("model parent"))
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    let size = model_artifact()?.size_bytes;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    file.write_all(bytes)
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    file.set_len(size)
        .map_err(|error| CedInstallError::new("install_failed", error.to_string(), 74))?;
    Ok(())
}

/// Write the journal-owned CED model and its model-only sidecar, without
/// constructing a writable engine directory.
pub fn write_ced_model_only(journal: &Path) -> Result<(), CedInstallError> {
    write_ced_model_bytes(journal, b"")?;
    write_model_sidecar(journal)?;
    Ok(())
}

/// Compile a load-only ced.cpp stub into `output`. Returns false when no C compiler is available.
pub fn compile_load_stub(output: &Path) -> bool {
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());
    if Command::new(&compiler).arg("--version").output().is_err() {
        return false;
    }
    let source = output.with_extension("c");
    if fs::write(&source, LOAD_STUB).is_err() {
        return false;
    }
    let mut command = Command::new(compiler);
    if std::env::consts::OS == "macos" {
        command.arg("-dynamiclib");
    } else {
        command.args(["-shared", "-fPIC"]);
    }
    let compiled = command
        .arg(&source)
        .arg("-o")
        .arg(output)
        .output()
        .expect("start C compiler");
    let _ = fs::remove_file(&source);
    assert!(
        compiled.status.success(),
        "compile CED load stub failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    true
}

/// SHA-256 of the installed model file.
pub fn ced_model_digest(journal: &Path) -> Result<String, String> {
    sha256_file(&ced_model_path(journal))
}

/// Complete install plus a host-native loadable stub at `key`'s library path.
pub fn write_ready_ced_install(journal: &Path, key: &str) -> bool {
    write_complete_ced_install(journal, key).expect("write complete CED install");
    compile_load_stub(&ced_library_path(journal, key))
}
