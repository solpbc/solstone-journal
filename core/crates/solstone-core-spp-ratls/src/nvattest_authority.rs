// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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

#[cfg(test)]
mod tests {
    use super::nvattest_platform_key;

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
}
