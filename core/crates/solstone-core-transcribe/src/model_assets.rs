// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bundled model asset discovery without a Python or helper-process fallback.

use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MODEL_ASSETS_ENV: &str = "SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR";
const SOURCE_ASSETS_RELATIVE: &str =
    "packages/solstone-journal-models/solstone_journal_models/assets";
const PACKAGE_ASSETS_RELATIVE: &str = "site-packages/solstone_journal_models/assets";

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
        }
    }
}

impl Error for ModelAssetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentExecutable { source } => Some(source),
            Self::OverrideInvalid { .. } | Self::AssetNotFound { .. } => None,
        }
    }
}

/// Resolve a named bundled transcription model asset.
pub fn resolve_model_asset(name: &str) -> Result<PathBuf, ModelAssetError> {
    let override_directory = env::var_os(MODEL_ASSETS_ENV).map(PathBuf::from);
    resolve_model_asset_from(
        name,
        override_directory.as_deref(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        env::current_exe(),
    )
}

fn resolve_model_asset_from(
    name: &str,
    override_directory: Option<&Path>,
    manifest_directory: &Path,
    current_executable: Result<PathBuf, io::Error>,
) -> Result<PathBuf, ModelAssetError> {
    if let Some(directory) = override_directory {
        return valid_asset(directory, name).ok_or_else(|| ModelAssetError::OverrideInvalid {
            asset: name.to_owned(),
            directory: directory.to_path_buf(),
        });
    }

    let source_directories = source_asset_directories(manifest_directory);
    if let Some(asset) = resolve_from_directories(name, &source_directories) {
        return Ok(asset);
    }

    let executable =
        current_executable.map_err(|source| ModelAssetError::CurrentExecutable { source })?;
    let installed_directories = installed_asset_directories(&executable);
    if let Some(asset) = resolve_from_directories(name, &installed_directories) {
        return Ok(asset);
    }

    let searched = source_directories
        .into_iter()
        .chain(installed_directories)
        .collect();
    Err(ModelAssetError::AssetNotFound {
        asset: name.to_owned(),
        searched,
    })
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

#[cfg(test)]
mod tests {
    use super::{ModelAssetError, resolve_model_asset_from};
    use std::fs;
    use std::io;
    use std::path::Path;

    const ASSET: &str = "silero_vad_v6.onnx";

    #[test]
    fn resolves_source_checkout_layout() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let asset_directory =
            root.join("packages/solstone-journal-models/solstone_journal_models/assets");
        write_asset(&asset_directory, ASSET);
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
        write_asset(&asset_directory, ASSET);

        let resolved = resolve_model_asset_from(
            ASSET,
            None,
            &root.join("source/core/crates/solstone-core-transcribe"),
            Ok(root.join("bin/solstone-transcribe")),
        )
        .unwrap();

        assert_eq!(resolved, asset_directory.join(ASSET));
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
        assert!(searched.contains(&installed_directory));
    }

    #[test]
    fn override_does_not_fall_through_to_other_layouts() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let source_directory =
            root.join("packages/solstone-journal-models/solstone_journal_models/assets");
        write_asset(&source_directory, ASSET);
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

    fn write_asset(directory: &Path, name: &str) {
        fs::create_dir_all(directory).unwrap();
        fs::write(directory.join(name), b"model").unwrap();
    }
}
