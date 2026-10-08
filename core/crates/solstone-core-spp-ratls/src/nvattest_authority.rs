// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_nvattest_authority::{
    AuthorityParseError, NvattestArtifactSpec, artifact_spec, parse,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthorityError {
    PlatformUnsupported,
    Malformed,
}

/// Windows owners can use confidential processing: it qualified on Windows
/// against the live service. Setting this back to false holds them again, and
/// then nothing offers the feature, installs a verifier or opens a channel on
/// Windows. It changes only in its own commit; no configuration, environment
/// variable or feature can lift or set the hold.
const WINDOWS_OWNER_USE_QUALIFIED: bool = true;

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
    let auth = parse(authority_json).map_err(|_| AuthorityError::Malformed)?;
    artifact_spec(&auth, platform).map_err(|e| match e {
        AuthorityParseError::PlatformUnsupported(_) => AuthorityError::PlatformUnsupported,
        AuthorityParseError::Malformed(_) => AuthorityError::Malformed,
    })
}

#[cfg(test)]
mod tests {
    use solstone_core_artifact_download::{PRODUCTION_DOWNLOAD_POLICY, origin_url};

    use solstone_core_nvattest_authority::AUTHORITY_JSON;

    use super::{AuthorityError, nvattest_platform_key, parse_nvattest_target};

    #[test]
    fn fixture_pins_compose_against_the_production_origin() {
        for platform in ["linux-x86_64", "linux-aarch64", "macos-arm64"] {
            let spec =
                parse_nvattest_target(AUTHORITY_JSON, platform).expect("fixture target parses");
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
        assert!(
            super::confidential_verifier_on_this_platform(),
            "every shipped journal target offers confidential processing"
        );
        for (os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("macos", "aarch64"),
            ("windows", "x86_64"),
        ] {
            assert!(super::verifier_on_platform(os, arch), "{os}-{arch}");
        }
        assert!(!super::verifier_on_platform("macos", "x86_64"));
    }

    #[test]
    fn no_shipped_platform_holds_its_owners_back() {
        for os in ["linux", "macos", "windows"] {
            assert!(!super::owner_use_held(os), "{os}");
        }
    }

    #[test]
    fn missing_target_is_platform_unsupported() {
        assert_eq!(
            parse_nvattest_target(AUTHORITY_JSON, "windows-x86_64"),
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
