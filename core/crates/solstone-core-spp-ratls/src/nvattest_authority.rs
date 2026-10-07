// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde::Deserialize;

pub(crate) const NVATTEST_AUTHORITY_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/nvattest_authority_v1.json"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NvattestArtifactSpec {
    pub platform: String,
    pub version: String,
    pub name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub url: String,
    pub origin_key: String,
    pub inventory: Vec<NvattestInventoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct NvattestInventoryEntry {
    pub relpath: String,
    pub executable: bool,
    pub kind: String,
    #[serde(default)]
    pub symlink_target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthorityError {
    PlatformUnsupported,
    Malformed,
}

#[derive(Deserialize)]
struct AuthorityFile {
    targets: BTreeMap<String, Target>,
}

#[derive(Deserialize)]
struct Target {
    artifact: ArtifactObject,
    inventory: Vec<NvattestInventoryEntry>,
    source: Source,
}

#[derive(Deserialize)]
struct ArtifactObject {
    name: String,
    sha256: String,
    size_bytes: Option<u64>,
    url: String,
}

#[derive(Deserialize)]
struct Source {
    version: String,
}

/// Windows owners can't use confidential processing until it is qualified on
/// Windows against the live service. While this is false, nothing offers the
/// feature, installs a verifier or opens a channel on Windows. It changes in its
/// own commit after that qualification; no configuration, environment variable
/// or feature can lift the hold.
const WINDOWS_OWNER_USE_QUALIFIED: bool = false;

/// Whether owners on `os` are held back from confidential processing even when
/// this build has a verifier for the platform. A held platform is not offered
/// the feature, installs no verifier and opens no channel to the service.
pub(crate) fn owner_use_held(os: &str) -> bool {
    os == "windows" && !WINDOWS_OWNER_USE_QUALIFIED
}

/// Whether confidential processing is offered on this platform: it has a
/// verifier target and its owners are not held back. Every other answer about
/// the hardware check comes from running it.
pub fn confidential_verifier_on_this_platform() -> bool {
    verifier_on_platform(std::env::consts::OS, std::env::consts::ARCH)
}

fn verifier_on_platform(os: &str, arch: &str) -> bool {
    nvattest_platform_key(os, arch).is_some() && !owner_use_held(os)
}

pub(crate) fn nvattest_platform_key(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        ("windows", "x86_64") => Some("windows-x86_64"),
        _ => None,
    }
}

pub(crate) fn parse_nvattest_target(
    authority_json: &str,
    platform: &str,
) -> Result<NvattestArtifactSpec, AuthorityError> {
    let authority: AuthorityFile =
        serde_json::from_str(authority_json).map_err(|_| AuthorityError::Malformed)?;
    let Some(target) = authority.targets.get(platform) else {
        return Err(AuthorityError::PlatformUnsupported);
    };
    let size_bytes = target
        .artifact
        .size_bytes
        .ok_or(AuthorityError::Malformed)?;
    let basename = target
        .artifact
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or(AuthorityError::Malformed)?;
    if basename != target.artifact.name {
        return Err(AuthorityError::Malformed);
    }
    Ok(NvattestArtifactSpec {
        platform: platform.to_owned(),
        version: target.source.version.clone(),
        name: target.artifact.name.clone(),
        sha256: target.artifact.sha256.clone(),
        size_bytes,
        url: target.artifact.url.clone(),
        origin_key: format!("providers/nvattest/{}", target.artifact.name),
        inventory: target.inventory.clone(),
    })
}

#[cfg(test)]
mod tests {
    use solstone_core_artifact_download::{PRODUCTION_DOWNLOAD_POLICY, origin_url};

    use super::{
        AuthorityError, NVATTEST_AUTHORITY_JSON, nvattest_platform_key, parse_nvattest_target,
    };

    #[test]
    fn fixture_pins_compose_against_the_production_origin() {
        for platform in ["linux-x86_64", "linux-aarch64", "macos-arm64"] {
            let spec = parse_nvattest_target(NVATTEST_AUTHORITY_JSON, platform)
                .expect("fixture target parses");
            assert_eq!(spec.platform, platform);
            assert_eq!(spec.version, "1.2.2-sol.6");
            assert_eq!(spec.origin_key, format!("providers/nvattest/{}", spec.name));
            assert_eq!(
                spec.url,
                origin_url(PRODUCTION_DOWNLOAD_POLICY.origin_base_url, &spec.origin_key)
            );
            assert_eq!(spec.sha256.len(), 64);
            assert!(spec.size_bytes > 0);
            assert!(
                spec.inventory
                    .iter()
                    .any(|entry| entry.relpath == "bin/nvattest" && entry.executable)
            );
        }
    }

    #[test]
    fn platform_key_matches_fixture_targets() {
        assert_eq!(
            nvattest_platform_key("linux", "x86_64"),
            Some("linux-x86_64")
        );
        assert_eq!(
            nvattest_platform_key("linux", "aarch64"),
            Some("linux-aarch64")
        );
        assert_eq!(
            nvattest_platform_key("macos", "aarch64"),
            Some("macos-arm64")
        );
        assert_eq!(
            nvattest_platform_key("windows", "x86_64"),
            Some("windows-x86_64")
        );
        assert_eq!(nvattest_platform_key("macos", "x86_64"), None);
        assert_eq!(nvattest_platform_key("linux", "arm"), None);
    }

    #[test]
    fn only_a_platform_with_a_verifier_target_offers_confidential_processing() {
        assert_eq!(
            super::confidential_verifier_on_this_platform(),
            !cfg!(windows),
            "every shipped journal target but Windows offers confidential processing"
        );
        assert!(!super::verifier_on_platform("windows", "x86_64"));
        if cfg!(unix) {
            for (os, arch) in [
                ("linux", "x86_64"),
                ("linux", "aarch64"),
                ("macos", "aarch64"),
            ] {
                assert!(super::verifier_on_platform(os, arch), "{os}-{arch}");
            }
        }
    }

    #[test]
    fn windows_owners_are_held_back_even_with_a_verifier_target() {
        assert!(super::owner_use_held("windows"));
        assert!(nvattest_platform_key("windows", "x86_64").is_some());
        assert!(!super::verifier_on_platform("windows", "x86_64"));
        for os in ["linux", "macos"] {
            assert!(!super::owner_use_held(os), "{os}");
        }
    }

    #[test]
    fn missing_target_is_platform_unsupported() {
        assert_eq!(
            parse_nvattest_target(NVATTEST_AUTHORITY_JSON, "windows-x86_64"),
            Err(AuthorityError::PlatformUnsupported)
        );
    }

    #[test]
    fn malformed_authority_is_distinct_from_platform_unsupported() {
        assert_eq!(
            parse_nvattest_target("not json", "linux-x86_64"),
            Err(AuthorityError::Malformed)
        );
        let missing_size_bytes = r#"{"targets":{"linux-x86_64":{"artifact":{"name":"payload.tar.xz","sha256":"ab","url":"https://updates.solstone.app/providers/nvattest/payload.tar.xz"},"inventory":[],"source":{"version":"test"}}}}"#;
        assert_eq!(
            parse_nvattest_target(missing_size_bytes, "linux-x86_64"),
            Err(AuthorityError::Malformed)
        );
        let basename_mismatch = r#"{"targets":{"linux-x86_64":{"artifact":{"name":"payload.tar.xz","sha256":"ab","size_bytes":1,"url":"https://updates.solstone.app/providers/nvattest/other.tar.xz"},"inventory":[],"source":{"version":"test"}}}}"#;
        assert_eq!(
            parse_nvattest_target(basename_mismatch, "linux-x86_64"),
            Err(AuthorityError::Malformed)
        );
    }
}
