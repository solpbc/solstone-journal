// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use solstone_core_installed_payload::{
    InstalledPackage, InstalledPayloadRefusal, code, guidance, locate_installed_package,
};

use crate::state::{AttestationFailure, AttestationFailureKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvattestPaths {
    pub binary: PathBuf,
    pub ca_bundle: PathBuf,
    pub library: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvattestRefusal {
    pub failure: AttestationFailure,
    pub detail: &'static str,
    pub guidance: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstalledRefusalKind {
    ManifestMissingRestart,
    ManifestMissingNormal,
    ManifestInvalid,
    UnsupportedLocation,
    WrongProduct,
    WrongTarget,
    RestartToFinishUpdate,
    MemberChanged,
    MemberMissing,
    UnexpectedFile,
    MemberUnreadable,
    UnsafePath,
}

impl InstalledRefusalKind {
    fn from_refusal(refusal: &InstalledPayloadRefusal) -> Self {
        if refusal.code == code::MANIFEST_MISSING {
            if refusal.guidance == guidance::RESTART_UPDATE {
                Self::ManifestMissingRestart
            } else {
                Self::ManifestMissingNormal
            }
        } else if refusal.code == code::MANIFEST_INVALID {
            Self::ManifestInvalid
        } else if refusal.code == code::UNSUPPORTED_LOCATION {
            Self::UnsupportedLocation
        } else if refusal.code == code::WRONG_PRODUCT {
            Self::WrongProduct
        } else if refusal.code == code::WRONG_TARGET {
            Self::WrongTarget
        } else if refusal.code == code::RESTART_TO_FINISH_UPDATE {
            Self::RestartToFinishUpdate
        } else if refusal.code == code::MEMBER_CHANGED {
            Self::MemberChanged
        } else if refusal.code == code::MEMBER_MISSING {
            Self::MemberMissing
        } else if refusal.code == code::UNEXPECTED_FILE {
            Self::UnexpectedFile
        } else if refusal.code == code::MEMBER_UNREADABLE {
            Self::MemberUnreadable
        } else if refusal.code == code::UNSAFE_PATH {
            Self::UnsafePath
        } else {
            unreachable!("unknown payload refusal code: {}", refusal.code);
        }
    }
}

pub fn refusal_for_installed_payload(refusal: &InstalledPayloadRefusal) -> NvattestRefusal {
    let kind = InstalledRefusalKind::from_refusal(refusal);
    match kind {
        InstalledRefusalKind::ManifestMissingRestart => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::MANIFEST_MISSING,
            guidance: guidance::RESTART_UPDATE,
        },
        InstalledRefusalKind::ManifestMissingNormal => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::MANIFEST_MISSING,
            guidance: guidance::MANIFEST_MISSING,
        },
        InstalledRefusalKind::ManifestInvalid => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::MANIFEST_INVALID,
            guidance: guidance::MANIFEST_INVALID,
        },
        InstalledRefusalKind::UnsupportedLocation => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::UNSUPPORTED_LOCATION,
            guidance: guidance::UNSUPPORTED_LOCATION,
        },
        InstalledRefusalKind::WrongProduct => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::WRONG_PRODUCT,
            guidance: guidance::WRONG_PRODUCT,
        },
        InstalledRefusalKind::WrongTarget => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::WRONG_TARGET,
            guidance: guidance::WRONG_TARGET,
        },
        InstalledRefusalKind::RestartToFinishUpdate => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::RESTART_TO_FINISH_UPDATE,
            guidance: guidance::RESTART_UPDATE,
        },
        InstalledRefusalKind::MemberChanged => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Failed,
                reason_code: "nvattest_integrity_failed",
            },
            detail: code::MEMBER_CHANGED,
            guidance: guidance::PACKAGE_MISMATCH,
        },
        InstalledRefusalKind::MemberMissing => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::MEMBER_MISSING,
            guidance: guidance::PACKAGE_MISMATCH,
        },
        InstalledRefusalKind::UnexpectedFile => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::UNEXPECTED_FILE,
            guidance: guidance::PACKAGE_MISMATCH,
        },
        InstalledRefusalKind::MemberUnreadable => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::MEMBER_UNREADABLE,
            guidance: guidance::MEMBER_UNREADABLE,
        },
        InstalledRefusalKind::UnsafePath => NvattestRefusal {
            failure: AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "nvattest_unavailable",
            },
            detail: code::UNSAFE_PATH,
            guidance: guidance::UNSAFE_PATH,
        },
    }
}

pub fn resolve_installed_nvattest(
    package: &InstalledPackage,
) -> Result<NvattestPaths, NvattestRefusal> {
    let binary = package
        .member("lib/solstone-nvattest/bin/nvattest")
        .map_err(|refusal| refusal_for_installed_payload(&refusal))?;
    let ca_bundle = package
        .member("lib/solstone-nvattest/share/ca/ca-bundle.pem")
        .map_err(|refusal| refusal_for_installed_payload(&refusal))?;

    #[cfg(target_os = "macos")]
    let lib_member = "lib/solstone-nvattest/lib/libnvat.1.dylib";
    #[cfg(not(target_os = "macos"))]
    let lib_member = "lib/solstone-nvattest/lib/libnvat.so.1";

    let library = package
        .member(lib_member)
        .map_err(|refusal| refusal_for_installed_payload(&refusal))?;

    Ok(NvattestPaths {
        binary,
        ca_bundle,
        library,
    })
}

pub fn resolve_nvattest_from_current_exe() -> Result<NvattestPaths, NvattestRefusal> {
    let current_exe = std::env::current_exe().map_err(|_| NvattestRefusal {
        failure: AttestationFailure {
            kind: AttestationFailureKind::Unreachable,
            reason_code: "nvattest_unavailable",
        },
        detail: code::UNSUPPORTED_LOCATION,
        guidance: guidance::UNSUPPORTED_LOCATION,
    })?;
    let root = locate_installed_package(
        &current_exe,
        solstone_core_installed_payload::host_executable_platform(),
    )
    .map_err(|refusal| refusal_for_installed_payload(&refusal))?;
    let package = InstalledPackage::admit(
        &root,
        solstone_core_installed_payload::COMPILED_VERSION,
        solstone_core_installed_payload::compiled_target(),
    )
    .map_err(|refusal| refusal_for_installed_payload(&refusal))?;
    resolve_installed_nvattest(&package)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_refusal(code: &'static str, guidance: &'static str) -> InstalledPayloadRefusal {
        InstalledPayloadRefusal {
            code,
            guidance,
            path: None,
            versions: None,
            targets: None,
            io: None,
        }
    }

    #[test]
    fn reason_map_matches_each_payload_refusal() {
        let cases = [
            (
                test_refusal(code::MANIFEST_MISSING, guidance::RESTART_UPDATE),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::MANIFEST_MISSING,
                guidance::RESTART_UPDATE,
            ),
            (
                test_refusal(code::MANIFEST_MISSING, guidance::MANIFEST_MISSING),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::MANIFEST_MISSING,
                guidance::MANIFEST_MISSING,
            ),
            (
                test_refusal(code::MANIFEST_INVALID, guidance::MANIFEST_INVALID),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::MANIFEST_INVALID,
                guidance::MANIFEST_INVALID,
            ),
            (
                test_refusal(code::UNSUPPORTED_LOCATION, guidance::UNSUPPORTED_LOCATION),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::UNSUPPORTED_LOCATION,
                guidance::UNSUPPORTED_LOCATION,
            ),
            (
                test_refusal(code::WRONG_PRODUCT, guidance::WRONG_PRODUCT),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::WRONG_PRODUCT,
                guidance::WRONG_PRODUCT,
            ),
            (
                test_refusal(code::WRONG_TARGET, guidance::WRONG_TARGET),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::WRONG_TARGET,
                guidance::WRONG_TARGET,
            ),
            (
                test_refusal(code::RESTART_TO_FINISH_UPDATE, guidance::RESTART_UPDATE),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::RESTART_TO_FINISH_UPDATE,
                guidance::RESTART_UPDATE,
            ),
            (
                test_refusal(code::MEMBER_CHANGED, guidance::PACKAGE_MISMATCH),
                "nvattest_integrity_failed",
                AttestationFailureKind::Failed,
                code::MEMBER_CHANGED,
                guidance::PACKAGE_MISMATCH,
            ),
            (
                test_refusal(code::MEMBER_MISSING, guidance::PACKAGE_MISMATCH),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::MEMBER_MISSING,
                guidance::PACKAGE_MISMATCH,
            ),
            (
                test_refusal(code::UNEXPECTED_FILE, guidance::PACKAGE_MISMATCH),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::UNEXPECTED_FILE,
                guidance::PACKAGE_MISMATCH,
            ),
            (
                test_refusal(code::MEMBER_UNREADABLE, guidance::MEMBER_UNREADABLE),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::MEMBER_UNREADABLE,
                guidance::MEMBER_UNREADABLE,
            ),
            (
                test_refusal(code::UNSAFE_PATH, guidance::UNSAFE_PATH),
                "nvattest_unavailable",
                AttestationFailureKind::Unreachable,
                code::UNSAFE_PATH,
                guidance::UNSAFE_PATH,
            ),
        ];

        for (refusal, reason, kind, detail, guidance) in cases {
            let mapped = refusal_for_installed_payload(&refusal);
            assert_eq!(mapped.failure.reason_code, reason);
            assert_eq!(mapped.failure.kind, kind);
            assert_eq!(mapped.detail, detail);
            assert_eq!(mapped.guidance, guidance);
        }
    }

    struct TestDir(PathBuf);
    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-spp-ratls-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn integrity_flipped_member_returns_package_mismatch() {
        use solstone_core_installed_payload::{
            COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, PRODUCT, compiled_target,
            render_installed_payload,
        };

        for target_member in [
            "lib/solstone-nvattest/bin/nvattest",
            if cfg!(target_os = "macos") {
                "lib/solstone-nvattest/lib/libnvat.1.dylib"
            } else {
                "lib/solstone-nvattest/lib/libnvat.so.1"
            },
            "lib/solstone-nvattest/share/ca/ca-bundle.pem",
        ] {
            let tmp = TestDir::new("tampered-member");
            let root = &tmp.0;
            let bin = root.join("lib/solstone-nvattest/bin/nvattest");
            let lib = if cfg!(target_os = "macos") {
                root.join("lib/solstone-nvattest/lib/libnvat.1.dylib")
            } else {
                root.join("lib/solstone-nvattest/lib/libnvat.so.1")
            };
            let ca = root.join("lib/solstone-nvattest/share/ca/ca-bundle.pem");
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
            std::fs::create_dir_all(ca.parent().unwrap()).unwrap();
            std::fs::write(&bin, b"binary placeholder bytes 1234567890").unwrap();
            std::fs::write(&lib, b"library placeholder bytes 1234567890").unwrap();
            std::fs::write(&ca, b"ca placeholder bytes 1234567890").unwrap();

            let manifest = render_installed_payload(
                root,
                PRODUCT,
                COMPILED_VERSION,
                compiled_target(),
                "test-commit",
            )
            .unwrap();
            let manifest_path = root.join(INSTALLED_PAYLOAD_MANIFEST);
            std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
            std::fs::write(&manifest_path, manifest).unwrap();

            let package =
                InstalledPackage::admit(root, COMPILED_VERSION, compiled_target()).unwrap();

            // Flip one byte of the target member keeping the same file length
            let file_to_tamper = root.join(target_member);
            let mut bytes = std::fs::read(&file_to_tamper).unwrap();
            bytes[0] ^= 0xff;
            std::fs::write(&file_to_tamper, bytes).unwrap();

            let refusal = resolve_installed_nvattest(&package).unwrap_err();
            assert_eq!(refusal.failure.reason_code, "nvattest_integrity_failed");
            assert_eq!(refusal.failure.kind, AttestationFailureKind::Failed);
            assert_eq!(refusal.guidance, guidance::PACKAGE_MISMATCH);
        }
    }

    #[test]
    fn integrity_newer_manifest_maps_to_restart_update() {
        use solstone_core_installed_payload::{
            COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, PRODUCT, compiled_target,
            render_installed_payload,
        };

        let tmp = TestDir::new("newer-manifest");
        let root = &tmp.0;
        let bin = root.join("lib/solstone-nvattest/bin/nvattest");
        let lib = if cfg!(target_os = "macos") {
            root.join("lib/solstone-nvattest/lib/libnvat.1.dylib")
        } else {
            root.join("lib/solstone-nvattest/lib/libnvat.so.1")
        };
        let ca = root.join("lib/solstone-nvattest/share/ca/ca-bundle.pem");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::create_dir_all(ca.parent().unwrap()).unwrap();
        std::fs::write(&bin, b"binary bytes").unwrap();
        std::fs::write(&lib, b"library bytes").unwrap();
        std::fs::write(&ca, b"ca bytes").unwrap();

        let manifest =
            render_installed_payload(root, PRODUCT, "999.0.0", compiled_target(), "test-commit")
                .unwrap();
        let manifest_path = root.join(INSTALLED_PAYLOAD_MANIFEST);
        std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        std::fs::write(&manifest_path, manifest).unwrap();

        let err = InstalledPackage::admit(root, COMPILED_VERSION, compiled_target()).unwrap_err();
        let mapped = refusal_for_installed_payload(&err);
        assert_eq!(mapped.failure.reason_code, "nvattest_unavailable");
        assert_eq!(mapped.failure.kind, AttestationFailureKind::Unreachable);
        assert_eq!(mapped.guidance, guidance::RESTART_UPDATE);
    }
}
