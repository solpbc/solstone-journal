// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows helper bootstrap, shared by speakers and Silero VAD.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use solstone_core_local::install::onnx_readiness::{
    WindowsOnnxPackage, verified_windows_onnx_package,
};
use solstone_core_win_dll_load::{LoadedOnnxRuntime, load_onnx_runtime};

static RUNTIME: Mutex<Option<(PathBuf, LoadedOnnxRuntime)>> = Mutex::new(None);

/// Recheck the current signed package before every helper operation. Only the
/// loaded DLL handle is retained; successful admission is never cached.
pub fn bootstrap_windows_onnx() -> Result<WindowsOnnxPackage, String> {
    let package = verified_windows_onnx_package()?;
    let mut runtime = RUNTIME
        .lock()
        .map_err(|_| "ONNX Runtime bootstrap failed after an earlier crash")?;
    if let Some((path, _)) = runtime.as_ref() {
        if path != &package.onnxruntime_library {
            return Err("ONNX Runtime is already bound to a different app payload".to_owned());
        }
        return Ok(package);
    }
    let loaded = load_onnx_runtime(&package.onnxruntime_library, ort::MINOR_VERSION)?;
    // The restricted loader above holds this exact module. Explicit init_from
    // prevents ort's default discovery from consulting ORT_DYLIB_PATH or PATH.
    let environment = ort::init_from(&package.onnxruntime_library)
        .map_err(|error| format!("ONNX Runtime initialization failed: {error}"))?;
    // init_from may silently retain a previously loaded G_ORT_LIB. Checking
    // environment.commit alone does not establish the runtime's actual ABI
    // binding: api()/info() can initialize that binding without an environment.
    let api = std::panic::catch_unwind(|| std::ptr::from_ref(ort::api()).cast())
        .map_err(|_| "ONNX Runtime API initialization failed".to_owned())?;
    if !loaded.matches_api(api) {
        return Err(
            "ONNX Runtime API is bound to a different library than the signed app payload"
                .to_owned(),
        );
    }
    if !environment.commit() {
        return Err("ONNX Runtime was configured before signed-payload bootstrap".to_owned());
    }
    *runtime = Some((package.onnxruntime_library.clone(), loaded));
    Ok(package)
}

pub fn loaded_windows_onnx_version() -> Result<String, String> {
    bootstrap_windows_onnx()?;
    let runtime = RUNTIME
        .lock()
        .map_err(|_| "ONNX Runtime bootstrap failed after an earlier crash")?;
    let (_, loaded) = runtime
        .as_ref()
        .ok_or("ONNX Runtime bootstrap did not retain its library")?;
    Ok(loaded.version().to_owned())
}

pub(crate) fn bootstrap_windows_speaker_model(model: &Path) -> Result<PathBuf, String> {
    let package = bootstrap_windows_onnx()?;
    for declared in [package.wespeaker_model, package.pyannote_model] {
        if solstone_core_local::install::windows_member_path::matches_declared_member_path(
            model, &declared,
        ) {
            return Ok(declared);
        }
    }
    Err("speaker model is not a declared member of the signed app payload".to_owned())
}

pub fn bootstrap_windows_vad_model(model: &Path) -> Result<PathBuf, String> {
    let package = bootstrap_windows_onnx()?;
    if !solstone_core_local::install::windows_member_path::matches_declared_member_path(
        model,
        &package.silero_vad_model,
    ) {
        return Err(
            "Silero VAD model is not a declared member of the signed app payload".to_owned(),
        );
    }
    Ok(package.silero_vad_model)
}
