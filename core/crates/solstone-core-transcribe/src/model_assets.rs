// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bundled model asset discovery without a Python or helper-process fallback.

use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const MODEL_ASSETS_ENV: &str = "SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR";
const SOURCE_ASSETS_RELATIVE: &str =
    "packages/solstone-journal-models/solstone_journal_models/assets";
const DATA_LIB_ASSETS_RELATIVE: &str = "lib/solstone_journal_models/assets";
const PACKAGE_ASSETS_RELATIVE: &str = "site-packages/solstone_journal_models/assets";

/// sha256 of the bundled WeSpeaker ResNet34 embedding graph.
pub const WESPEAKER_RESNET34_SHA256: &str =
    "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94";
/// sha256 of the bundled pyannote segmentation graph.
pub const PYANNOTE_SEGMENTATION_SHA256: &str =
    "057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25";
/// sha256 of the bundled Silero VAD v6 graph.
pub const SILERO_VAD_V6_SHA256: &str =
    "4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2";

/// Failure to locate a bundled transcription model asset.
#[derive(Debug)]
pub enum ModelAssetError {
    /// The explicit asset directory override did not provide the requested file.
    OverrideInvalid { asset: String, directory: PathBuf },
    /// The installed-layout probe could not determine this executable's path.
    CurrentExecutable { source: io::Error },
    /// No searched asset directory contained a nonempty regular file with this name.
    AssetNotFound {
        asset: String,
        searched: Vec<PathBuf>,
    },
    /// A discovered model asset did not match the pinned digest.
    DigestMismatch {
        asset: String,
        path: PathBuf,
        expected: &'static str,
        actual: String,
    },
}

impl fmt::Display for ModelAssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OverrideInvalid { asset, directory } => write!(
                formatter,
                "{MODEL_ASSETS_ENV}={:?} does not provide nonempty model asset {asset:?}",
                directory
            ),
            Self::CurrentExecutable { source } => {
                write!(
                    formatter,
                    "could not determine current executable: {source}"
                )
            }
            Self::AssetNotFound { asset, searched } => write!(
                formatter,
                "could not find nonempty model asset {asset:?}; searched {}",
                searched
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::DigestMismatch {
                asset,
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "model asset {asset:?} at {} has sha256 {actual}, expected {expected}",
                path.display()
            ),
        }
    }
}

impl Error for ModelAssetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentExecutable { source } => Some(source),
            Self::OverrideInvalid { .. }
            | Self::AssetNotFound { .. }
            | Self::DigestMismatch { .. } => None,
        }
    }
}

/// Resolve a named bundled transcription model asset.
pub fn resolve_model_asset(name: &str) -> Result<PathBuf, ModelAssetError> {
    resolve_model_asset_path(name).and_then(|path| verify_digest(name, path))
}

/// Locate a model asset using the same layout resolution as [`resolve_model_asset`].
///
/// This deliberately does not verify bytes. It is used only to construct a
/// generation proof before deciding whether an inherited proof can be borrowed.
pub(crate) fn resolve_model_asset_path(name: &str) -> Result<PathBuf, ModelAssetError> {
    let override_directory = env::var_os(MODEL_ASSETS_ENV).map(PathBuf::from);
    resolve_model_asset_path_from(
        name,
        override_directory.as_deref(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        env::current_exe(),
    )
}

fn resolve_model_asset_path_from(
    name: &str,
    override_directory: Option<&Path>,
    manifest_directory: &Path,
    current_executable: Result<PathBuf, io::Error>,
) -> Result<PathBuf, ModelAssetError> {
    if let Some(directory) = override_directory {
        let asset =
            valid_asset(directory, name).ok_or_else(|| ModelAssetError::OverrideInvalid {
                asset: name.to_owned(),
                directory: directory.to_path_buf(),
            })?;
        return Ok(asset);
    }

    let source_directories = source_asset_directories(manifest_directory);
    if let Some(asset) = resolve_from_directories(name, &source_directories) {
        return Ok(asset);
    }

    let executable =
        current_executable.map_err(|source| ModelAssetError::CurrentExecutable { source })?;
    let executable_relative_directories = executable_relative_asset_directories(&executable);
    if let Some(asset) = resolve_from_directories(name, &executable_relative_directories) {
        return Ok(asset);
    }

    let installed_directories = installed_asset_directories(&executable);
    if let Some(asset) = resolve_from_directories(name, &installed_directories) {
        return Ok(asset);
    }

    let searched = source_directories
        .into_iter()
        .chain(executable_relative_directories)
        .chain(installed_directories)
        .collect();
    Err(ModelAssetError::AssetNotFound {
        asset: name.to_owned(),
        searched,
    })
}

#[cfg(test)]
fn resolve_model_asset_from(
    name: &str,
    override_directory: Option<&Path>,
    manifest_directory: &Path,
    current_executable: Result<PathBuf, io::Error>,
) -> Result<PathBuf, ModelAssetError> {
    resolve_model_asset_path_from(
        name,
        override_directory,
        manifest_directory,
        current_executable,
    )
    .and_then(|path| verify_digest(name, path))
}

fn executable_relative_asset_directories(executable: &Path) -> Vec<PathBuf> {
    executable
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .map(|root| root.join(DATA_LIB_ASSETS_RELATIVE))
        .collect()
}

fn source_asset_directories(manifest_directory: &Path) -> Vec<PathBuf> {
    manifest_directory
        .ancestors()
        .map(|ancestor| ancestor.join(SOURCE_ASSETS_RELATIVE))
        .collect()
}

fn installed_asset_directories(executable: &Path) -> Vec<PathBuf> {
    executable
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .flat_map(|root| python_asset_directories(&root.join("lib")))
        .collect()
}

fn python_asset_directories(library_directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(library_directory) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let is_python_directory = entry
                .file_type()
                .ok()
                .is_some_and(|file_type| file_type.is_dir())
                && entry.file_name().to_string_lossy().starts_with("python3.");
            is_python_directory.then(|| entry.path().join(PACKAGE_ASSETS_RELATIVE))
        })
        .collect()
}

fn resolve_from_directories(name: &str, directories: &[PathBuf]) -> Option<PathBuf> {
    directories
        .iter()
        .find_map(|directory| valid_asset(directory, name))
}

fn valid_asset(directory: &Path, name: &str) -> Option<PathBuf> {
    let asset = directory.join(name);
    fs::metadata(&asset)
        .ok()
        .filter(|metadata| metadata.is_file() && metadata.len() > 0)
        .map(|_| asset)
}

fn verify_digest(name: &str, path: PathBuf) -> Result<PathBuf, ModelAssetError> {
    let expected = expected_sha256(name);
    let actual = sha256_file(&path).unwrap_or_else(|error| format!("<unreadable: {error}>"));
    if actual == expected {
        Ok(path)
    } else {
        Err(ModelAssetError::DigestMismatch {
            asset: name.to_owned(),
            path,
            expected,
            actual,
        })
    }
}

fn expected_sha256(name: &str) -> &'static str {
    match name {
        "silero_vad_v6.onnx" => SILERO_VAD_V6_SHA256,
        "wespeaker-resnet34-256.onnx" => WESPEAKER_RESNET34_SHA256,
        "pyannote-segmentation-3.0.onnx" => PYANNOTE_SEGMENTATION_SHA256,
        _ => panic!("unknown bundled transcription model asset {name:?}"),
    }
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::{
        DATA_LIB_ASSETS_RELATIVE, ModelAssetError, PYANNOTE_SEGMENTATION_SHA256,
        SILERO_VAD_V6_SHA256, resolve_model_asset_from,
    };
    use serde_json::Value;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    const ASSET: &str = "silero_vad_v6.onnx";

    #[test]
    fn resolves_source_checkout_layout() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let asset_directory =
            root.join("packages/solstone-journal-models/solstone_journal_models/assets");
        write_real_asset(&asset_directory, ASSET);
        let manifest_directory = root.join("core/crates/solstone-core-transcribe");

        let resolved = resolve_model_asset_from(
            ASSET,
            None,
            &manifest_directory,
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap();

        assert_eq!(resolved, asset_directory.join(ASSET));
    }

    #[test]
    fn resolves_installed_package_layout() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let asset_directory =
            root.join("lib/python3.13/site-packages/solstone_journal_models/assets");
        // AC4: Python package layouts remain a valid read-compatibility path.
        write_real_asset(&asset_directory, ASSET);

        let resolved = resolve_model_asset_from(
            ASSET,
            None,
            &root.join("source/core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap();

        assert_eq!(resolved, asset_directory.join(ASSET));
    }

    /// AC1/AC2 use injected paths because this checkout's compile-time
    /// `CARGO_MANIFEST_DIR` always reaches the real committed model assets.
    /// Copying a compiled binary cannot isolate this branch without deleting
    /// repository content or recompiling from a relocated source tree.
    #[test]
    fn resolves_executable_relative_layout_without_python_package_layout() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let asset_directory = root.join(DATA_LIB_ASSETS_RELATIVE);
        write_real_asset(&asset_directory, ASSET);

        let resolved = resolve_model_asset_from(
            ASSET,
            None,
            &root.join("source/core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap();

        assert_eq!(resolved, asset_directory.join(ASSET));
        assert!(!has_python_package_layout(root));
    }

    #[test]
    fn missing_assets_search_executable_relative_paths_before_python_layouts() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let python_directory =
            root.join("lib/python3.13/site-packages/solstone_journal_models/assets");
        fs::create_dir_all(&python_directory).unwrap();

        let error = resolve_model_asset_from(
            ASSET,
            None,
            &root.join("source/core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap_err();

        let ModelAssetError::AssetNotFound { searched, .. } = error else {
            panic!("expected missing asset error");
        };
        let first_python = searched
            .iter()
            .position(|path| path.to_string_lossy().contains("python3."))
            .expect("python layout candidate");
        assert!(searched[..first_python].iter().all(|path| {
            let path = path.to_string_lossy();
            !path.contains("python3.") && !path.contains("site-packages")
        }));
        assert!(searched[first_python..].iter().any(|path| {
            let path = path.to_string_lossy();
            path.contains("python3.") && path.contains("site-packages")
        }));
        assert!(
            searched[..first_python]
                .iter()
                .any(|path| path == &root.join(DATA_LIB_ASSETS_RELATIVE))
        );
    }

    #[test]
    fn missing_asset_records_all_existing_layout_candidates() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let source_directory =
            root.join("packages/solstone-journal-models/solstone_journal_models/assets");
        fs::create_dir_all(&source_directory).unwrap();
        let installed_directory =
            root.join("lib/python3.13/site-packages/solstone_journal_models/assets");
        fs::create_dir_all(&installed_directory).unwrap();
        let executable_relative_directory = root.join(DATA_LIB_ASSETS_RELATIVE);
        fs::create_dir_all(&executable_relative_directory).unwrap();

        let error = resolve_model_asset_from(
            ASSET,
            None,
            &root.join("core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap_err();

        let ModelAssetError::AssetNotFound { asset, searched } = error else {
            panic!("expected missing asset error");
        };
        assert_eq!(asset, ASSET);
        assert!(searched.contains(&source_directory));
        assert!(searched.contains(&executable_relative_directory));
        assert!(searched.contains(&installed_directory));
    }

    #[test]
    fn override_does_not_fall_through_to_other_layouts() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let source_directory =
            root.join("packages/solstone-journal-models/solstone_journal_models/assets");
        write_real_asset(&source_directory, ASSET);
        let override_directory = root.join("override");

        let error = resolve_model_asset_from(
            ASSET,
            Some(&override_directory),
            &root.join("core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap_err();

        assert!(matches!(error, ModelAssetError::OverrideInvalid { .. }));
    }

    #[test]
    fn current_executable_failure_is_typed() {
        let temporary = tempfile::tempdir().unwrap();
        let error = resolve_model_asset_from(
            ASSET,
            None,
            &temporary
                .path()
                .join("core/crates/solstone-core-transcribe"),
            Err(io::Error::other("unavailable")),
        )
        .unwrap_err();

        assert!(matches!(error, ModelAssetError::CurrentExecutable { .. }));
    }

    #[test]
    fn missing_and_corrupt_assets_are_distinct_and_corruption_does_not_fall_through() {
        let missing = tempfile::tempdir().unwrap();
        let missing_error = resolve_model_asset_from(
            ASSET,
            None,
            &missing
                .path()
                .join("source/core/crates/solstone-core-transcribe"),
            Ok(missing.path().join("bin/solstone-transcribe")),
        )
        .unwrap_err();
        assert!(matches!(
            missing_error,
            ModelAssetError::AssetNotFound { .. }
        ));

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let corrupt_directory =
            root.join("packages/solstone-journal-models/solstone_journal_models/assets");
        let corrupt_path = write_corrupted_real_asset(&corrupt_directory, ASSET);
        let later_directory = root.join(DATA_LIB_ASSETS_RELATIVE);
        write_real_asset(&later_directory, ASSET);

        let error = resolve_model_asset_from(
            ASSET,
            None,
            &root.join("core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap_err();

        let ModelAssetError::DigestMismatch {
            asset,
            path,
            expected,
            actual,
        } = error
        else {
            panic!("expected digest mismatch");
        };
        assert_eq!(asset, ASSET);
        assert_eq!(path, corrupt_path);
        assert_eq!(expected, SILERO_VAD_V6_SHA256);
        assert_ne!(actual, expected);
    }

    #[test]
    fn corrupted_override_reports_digest_mismatch_not_override_invalid() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let override_directory = root.join("override");
        let corrupt_path = write_corrupted_real_asset(&override_directory, ASSET);
        let source_directory =
            root.join("packages/solstone-journal-models/solstone_journal_models/assets");
        write_real_asset(&source_directory, ASSET);

        let error = resolve_model_asset_from(
            ASSET,
            Some(&override_directory),
            &root.join("core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap_err();

        let ModelAssetError::DigestMismatch {
            path,
            expected,
            actual,
            ..
        } = error
        else {
            panic!("expected digest mismatch");
        };
        assert_eq!(path, corrupt_path);
        assert_eq!(expected, SILERO_VAD_V6_SHA256);
        assert_ne!(actual, expected);
    }

    #[test]
    fn speaker_fixture_digests_match_canonical_pyannote_digest() {
        for fixture in ["speaker_stage_boundaries.json", "speaker_filterbank.json"] {
            assert_eq!(
                fixture_overlap_detector_sha256(fixture),
                PYANNOTE_SEGMENTATION_SHA256,
                "{fixture}"
            );
        }
    }

    fn committed_asset_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("packages/solstone-journal-models/solstone_journal_models/assets")
            .join(name)
    }

    fn write_real_asset(directory: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(directory).unwrap();
        let destination = directory.join(name);
        fs::copy(committed_asset_path(name), &destination).unwrap();
        destination
    }

    fn write_corrupted_real_asset(directory: &Path, name: &str) -> PathBuf {
        let destination = write_real_asset(directory, name);
        let mut bytes = fs::read(&destination).unwrap();
        bytes[0] ^= 1;
        fs::write(&destination, bytes).unwrap();
        destination
    }

    fn has_python_package_layout(directory: &Path) -> bool {
        fs::read_dir(directory).unwrap().flatten().any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("python3.")
                || name == "site-packages"
                || entry.file_type().unwrap().is_dir() && has_python_package_layout(&entry.path())
        })
    }

    fn fixture_overlap_detector_sha256(name: &str) -> String {
        let fixture: Value = serde_json::from_slice(
            &fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures")
                    .join(name),
            )
            .unwrap(),
        )
        .unwrap();
        fixture["identity"]["source_constants"]["encoder_config"]["OVERLAP_DETECTOR_SHA256"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}
