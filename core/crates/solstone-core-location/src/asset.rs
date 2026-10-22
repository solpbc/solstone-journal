// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Loading the pinned area tier from the journal's package.
//!
//! Desktop packages carry the model asset beside the speech models at
//! `lib/solstone_journal_models/assets/geo/`. Mobile libraries embed the same
//! pinned bytes because their app process does not use that executable layout.

use std::fmt;
use std::fs;
use std::io;
#[cfg(any(test, not(any(target_os = "android", target_os = "ios"))))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::area::{AreaTier, AreaTierError};

/// The area tier's file name under the assets directory.
pub const AREA_TIER_ASSET: &str = "geo/area-tier.bin";
/// sha256 of the bundled area tier (GeoNames cities500 and Natural Earth).
pub const AREA_TIER_SHA256: &str =
    "ed233aa4129b23092154cfae5680df5f7200f59b89326e72e6bd88215de284ce";

const SOURCE_ASSETS_RELATIVE: &str = "core/models/assets";
const INSTALLED_ASSETS_RELATIVE: &str = "lib/solstone_journal_models/assets";

/// The bundled area tier could not be found or did not match its pin.
#[derive(Debug)]
pub enum AssetError {
    NotFound { searched: Vec<PathBuf> },
    Unreadable { path: PathBuf, source: io::Error },
    DigestMismatch { path: PathBuf, actual: String },
}

impl fmt::Display for AssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { searched } => write!(
                formatter,
                "the bundled area tier {AREA_TIER_ASSET} was not found (searched {searched:?})"
            ),
            Self::Unreadable { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::DigestMismatch { path, actual } => write!(
                formatter,
                "{} has sha256 {actual}, expected {AREA_TIER_SHA256}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AssetError {}

fn candidates(manifest_directory: &Path, executable: Option<&Path>) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = manifest_directory
        .ancestors()
        .map(|ancestor| ancestor.join(SOURCE_ASSETS_RELATIVE))
        .collect();
    if let Some(executable) = executable {
        directories.extend(
            executable
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .map(|root| root.join(INSTALLED_ASSETS_RELATIVE)),
        );
    }
    directories
        .into_iter()
        .map(|directory| directory.join(AREA_TIER_ASSET))
        .collect()
}

/// Locate the area tier file: the checkout's copy when running from source,
/// otherwise the copy installed beside this executable.
pub fn resolve_area_tier_path() -> Result<PathBuf, AssetError> {
    let executable = std::env::current_exe().ok();
    resolve_from(Path::new(env!("CARGO_MANIFEST_DIR")), executable.as_deref())
}

fn resolve_from(
    manifest_directory: &Path,
    executable: Option<&Path>,
) -> Result<PathBuf, AssetError> {
    let searched = candidates(manifest_directory, executable);
    searched
        .iter()
        .find(|path| {
            fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
        })
        .cloned()
        .ok_or(AssetError::NotFound { searched })
}

#[cfg(any(test, not(any(target_os = "android", target_os = "ios"))))]
fn read_verified(path: &Path) -> Result<Vec<u8>, AssetError> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|source| AssetError::Unreadable {
            path: path.to_path_buf(),
            source,
        })?;
    verify_bytes(path, &bytes)?;
    Ok(bytes)
}

fn verify_bytes(path: &Path, bytes: &[u8]) -> Result<(), AssetError> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != AREA_TIER_SHA256 {
        return Err(AssetError::DigestMismatch {
            path: path.to_path_buf(),
            actual,
        });
    }
    Ok(())
}

static AREA_TIER: OnceLock<Result<AreaTier, String>> = OnceLock::new();

#[cfg(any(test, target_os = "android", target_os = "ios"))]
fn embedded_area_tier() -> Result<AreaTier, String> {
    const BYTES: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../models/assets/geo/area-tier.bin"
    ));
    verify_bytes(Path::new(AREA_TIER_ASSET), BYTES).map_err(|error| error.to_string())?;
    AreaTier::parse(BYTES).map_err(|error| error.to_string())
}

/// The bundled area tier, read and verified once per process.
pub fn area_tier() -> Result<&'static AreaTier, AreaTierError> {
    AREA_TIER
        .get_or_init(|| {
            #[cfg(any(target_os = "android", target_os = "ios"))]
            {
                embedded_area_tier()
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let path = resolve_area_tier_path().map_err(|error| error.to_string())?;
                let bytes = read_verified(&path).map_err(|error| error.to_string())?;
                AreaTier::parse(&bytes).map_err(|error| error.to_string())
            }
        })
        .as_ref()
        .map_err(|detail| AreaTierError::Unavailable(detail.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installed_layout_resolves_beside_the_executable() {
        let root = tempfile::tempdir().unwrap();
        let asset = root
            .path()
            .join(INSTALLED_ASSETS_RELATIVE)
            .join(AREA_TIER_ASSET);
        fs::create_dir_all(asset.parent().unwrap()).unwrap();
        fs::write(&asset, b"x").unwrap();
        let executable = root.path().join("bin/solstone-core");
        let resolved = resolve_from(Path::new("/nonexistent/manifest"), Some(&executable)).unwrap();
        assert_eq!(resolved, asset);
    }

    #[test]
    fn a_missing_asset_names_where_it_looked() {
        let root = tempfile::tempdir().unwrap();
        let error = resolve_from(root.path(), Some(&root.path().join("bin/x"))).unwrap_err();
        assert!(matches!(error, AssetError::NotFound { searched } if !searched.is_empty()));
    }

    #[test]
    fn the_checkout_copy_matches_its_pin() {
        let path = resolve_area_tier_path().unwrap();
        read_verified(&path).unwrap();
    }

    #[test]
    fn mobile_embedded_tier_names_real_geography() {
        let tier = embedded_area_tier().expect("pinned mobile area tier");
        assert_eq!(
            tier.name_point(39.7555, -105.2211).words(),
            ["Golden", "Colorado", "United States"]
        );
    }

    #[test]
    fn embedded_verifier_refuses_changed_bytes() {
        assert!(matches!(
            verify_bytes(Path::new(AREA_TIER_ASSET), b"SLAT"),
            Err(AssetError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn altered_bytes_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("area-tier.bin");
        fs::write(&path, b"SLAT").unwrap();
        assert!(matches!(
            read_verified(&path),
            Err(AssetError::DigestMismatch { .. })
        ));
    }
}
