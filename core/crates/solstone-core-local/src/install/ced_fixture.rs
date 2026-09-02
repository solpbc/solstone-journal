// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared on-disk CED install used by tests in the consumer crates.
//!
//! One definition of a correct install so `local`, `sound-tags`,
//! `install-models`, `check`, and `health` tests do not each invent a layout.
//!
//! This module no longer compiles a native `libced.so` load stub (it used to,
//! as `compile_load_stub`/`write_ready_ced_install`): the deep engine probe
//! that stub proved runs out of process now, through
//! `solstone-core-ced-analyze` (Brief D). A "ready" verdict is obtained
//! either through `ced_readiness::evaluate_ced_readiness_against_with_probe`
//! (a caller-supplied probe closure) or, for a genuine `dlopen` proof, in
//! `solstone-core-ced-analyze`'s own tests, which compile
//! `tests/fixtures/ced_stub.c`.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::ced_install::{
    CedInstallError, ced_model_path, engine_dir, library_name, model_artifact, write_sidecar,
};
use super::manifest::sha256_file;

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

/// SHA-256 of the installed model file.
pub fn ced_model_digest(journal: &Path) -> Result<String, String> {
    sha256_file(&ced_model_path(journal))
}
