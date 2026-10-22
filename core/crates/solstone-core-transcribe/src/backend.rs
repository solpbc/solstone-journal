// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-input collection around the shared pure STT backend choice.

pub(crate) mod confidential;
#[cfg(unix)]
pub(crate) mod parakeet_coreml;
pub(crate) mod parakeet_cpp;

use std::env;

use solstone_core_system::stt_backend_choice::{
    STT_BACKENDS, STT_SURFACE, resolve_stt_backend_choice, stt_host_policy,
};

use crate::TranscribeError;

pub(crate) const KNOWN_BACKENDS: [&str; 3] = STT_BACKENDS;

/// Non-fatal backend-selection information for the caller to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BackendWarning {
    /// An explicit config or CLI backend is not registered in this build.
    UnknownExplicitBackend { backend: String },
    /// An explicit CoreML Parakeet choice is below the local memory floor.
    ExplicitParakeetBelowFloor,
}

/// The pure backend choice plus observable warning conditions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackendResolution {
    pub(crate) backend: String,
    pub(crate) warnings: Vec<BackendWarning>,
}

/// Read available RAM through the shared host probe.
pub(crate) fn read_available_bytes() -> Option<u64> {
    solstone_core_system::memory_admission::available_physical_bytes()
}

/// Return the local STT floor for the current OS and architecture.
pub(crate) fn platform_floor_bytes() -> Option<u64> {
    platform_floor_bytes_for(env::consts::OS, env::consts::ARCH)
}

/// Return the local STT backend available on the current OS and architecture.
pub(crate) fn local_stt_backend() -> Option<&'static str> {
    local_stt_backend_for(env::consts::OS, env::consts::ARCH)
}

/// Collect warning conditions and delegate the actual policy choice to core-system.
pub(crate) fn resolve_default_backend(
    explicit_backend: Option<&str>,
    local_backend: Option<&str>,
    available_bytes: Option<u64>,
    floor_bytes: Option<u64>,
    confidential_lane_active: bool,
    confidential_audio_enabled: bool,
) -> Result<BackendResolution, TranscribeError> {
    let mut warnings = Vec::new();
    let explicit_backend = normalize_explicit_backend(explicit_backend, &mut warnings);
    let backend = resolve_stt_backend_choice(
        explicit_backend,
        available_bytes,
        floor_bytes,
        local_backend,
        confidential_lane_active,
        confidential_audio_enabled,
    );
    if backend == STT_SURFACE {
        return Err(TranscribeError::SttSurface {
            available_bytes,
            floor_bytes,
        });
    }
    if let Some(warning) =
        warn_if_local_below_floor(explicit_backend, &backend, available_bytes, floor_bytes)
    {
        warnings.push(warning);
    }
    Ok(BackendResolution { backend, warnings })
}

fn platform_floor_bytes_for(os: &str, arch: &str) -> Option<u64> {
    stt_host_policy(os, arch).local_floor_bytes
}

fn local_stt_backend_for(os: &str, arch: &str) -> Option<&'static str> {
    stt_host_policy(os, arch).local_backend
}

fn normalize_explicit_backend<'a>(
    explicit_backend: Option<&'a str>,
    warnings: &mut Vec<BackendWarning>,
) -> Option<&'a str> {
    match explicit_backend {
        Some(backend) if !KNOWN_BACKENDS.contains(&backend) => {
            warnings.push(BackendWarning::UnknownExplicitBackend {
                backend: backend.to_owned(),
            });
            None
        }
        backend => backend,
    }
}

fn warn_if_local_below_floor(
    explicit_backend: Option<&str>,
    backend: &str,
    available_bytes: Option<u64>,
    floor_bytes: Option<u64>,
) -> Option<BackendWarning> {
    if explicit_backend.is_none() || !matches!(backend, "parakeet" | "parakeet-cpp") {
        return None;
    }
    (backend == "parakeet"
        && available_bytes
            .zip(floor_bytes)
            .is_some_and(|(available, floor)| available < floor))
    .then_some(BackendWarning::ExplicitParakeetBelowFloor)
}

#[cfg(test)]
mod tests {
    use super::{
        BackendWarning, local_stt_backend_for, normalize_explicit_backend,
        platform_floor_bytes_for, resolve_default_backend, warn_if_local_below_floor,
    };
    use crate::TranscribeError;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn host_aliases_preserve_auto_admission_and_explicit_overrides() {
        for (os, arch, floor) in [
            ("macos", "aarch64", Some(0)),
            ("darwin", "arm64", Some(0)),
            ("linux", "x86_64", Some(4 * GIB)),
            ("linux", "amd64", Some(4 * GIB)),
            ("linux", "x64", Some(4 * GIB)),
            ("linux", "aarch64", Some(4 * GIB)),
            ("linux", "arm64", Some(4 * GIB)),
            ("windows", "x86_64", Some(4 * GIB)),
            ("macos", "x86_64", None),
            ("windows", "aarch64", Some(4 * GIB)),
            ("windows", "armv7l", None),
            ("linux", "armv7l", None),
            ("unknown", "x86_64", None),
        ] {
            let local = local_stt_backend_for(os, arch);
            assert_eq!(platform_floor_bytes_for(os, arch), floor, "{os}/{arch}");
            assert_eq!(local, floor.map(|_| "parakeet"), "{os}/{arch}");
            for available in [None, Some(0), Some(4 * GIB - 1), Some(4 * GIB)] {
                let resolution =
                    resolve_default_backend(None, local, available, floor, false, false);
                let fits = available
                    .zip(floor)
                    .is_some_and(|(bytes, floor)| bytes >= floor);
                assert_eq!(resolution.is_ok(), fits, "{os}/{arch}, {available:?}");
            }
            for explicit in ["parakeet", "parakeet-cpp"] {
                assert_eq!(
                    resolve_default_backend(Some(explicit), local, None, floor, false, false)
                        .unwrap()
                        .backend,
                    explicit
                );
            }
        }
    }

    #[test]
    fn linux_platform_floor_is_four_gib() {
        assert_eq!(platform_floor_bytes_for("linux", "x86_64"), Some(4 * GIB));
    }

    #[test]
    fn darwin_arm64_platform_floor_is_zero() {
        assert_eq!(platform_floor_bytes_for("darwin", "arm64"), Some(0));
    }

    #[test]
    fn macos_aarch64_platform_floor_is_zero() {
        assert_eq!(platform_floor_bytes_for("macos", "aarch64"), Some(0));
    }

    #[test]
    fn darwin_default_admits_local_without_a_free_ram_reservation() {
        let local = local_stt_backend_for("darwin", "arm64");
        let floor = platform_floor_bytes_for("darwin", "arm64");
        assert_eq!(
            resolve_default_backend(None, local, Some(1), floor, false, false)
                .unwrap()
                .backend,
            "parakeet"
        );
        assert!(matches!(
            resolve_default_backend(None, local, None, floor, false, false),
            Err(TranscribeError::SttSurface { .. })
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn running_macos_default_selects_coreml_when_memory_is_available() {
        let resolution = resolve_default_backend(
            None,
            super::local_stt_backend(),
            super::read_available_bytes(),
            super::platform_floor_bytes(),
            false,
            false,
        )
        .expect("this Mac has enough available memory for CoreML transcription");

        assert_eq!(resolution.backend, "parakeet");
    }

    #[test]
    fn local_stt_backend_supports_darwin_arm64() {
        assert_eq!(local_stt_backend_for("darwin", "arm64"), Some("parakeet"));
    }

    #[test]
    fn local_stt_backend_supports_macos_aarch64() {
        assert_eq!(local_stt_backend_for("macos", "aarch64"), Some("parakeet"));
    }

    #[test]
    fn local_stt_backend_supports_linux_x86_64() {
        assert_eq!(local_stt_backend_for("linux", "x86_64"), Some("parakeet"));
    }

    #[test]
    fn local_stt_backend_supports_linux_aarch64() {
        assert_eq!(local_stt_backend_for("linux", "aarch64"), Some("parakeet"));
    }

    #[test]
    fn local_stt_backend_supports_linux_arm64() {
        assert_eq!(local_stt_backend_for("linux", "arm64"), Some("parakeet"));
    }

    #[test]
    fn local_stt_backend_rejects_unsupported_platforms() {
        assert_eq!(local_stt_backend_for("darwin", "x86_64"), None);
        assert_eq!(local_stt_backend_for("linux", "armv7l"), None);
        assert_eq!(local_stt_backend_for("windows", "armv7l"), None);
    }

    #[test]
    fn windows_default_and_explicit_cpu_selection_preserve_resource_refusal() {
        for arch in ["x86_64", "aarch64"] {
            let local = local_stt_backend_for("windows", arch);
            let floor = platform_floor_bytes_for("windows", arch);
            assert_eq!(local, Some("parakeet"));
            assert_eq!(floor, Some(4 * GIB));
            assert_eq!(
                resolve_default_backend(None, local, floor, floor, false, false)
                    .unwrap()
                    .backend,
                "parakeet"
            );
            for available in [None, Some(0), Some(4 * GIB - 1)] {
                assert!(matches!(
                    resolve_default_backend(None, local, available, floor, false, false),
                    Err(TranscribeError::SttSurface { .. })
                ));
            }
            for explicit in ["parakeet", "parakeet-cpp"] {
                assert_eq!(
                    resolve_default_backend(Some(explicit), local, None, floor, false, false)
                        .unwrap()
                        .backend,
                    explicit
                );
            }
        }
    }

    #[test]
    fn unknown_explicit_backend_is_downgraded_with_warning() {
        let mut warnings = Vec::new();

        assert_eq!(
            normalize_explicit_backend(Some("not-a-backend"), &mut warnings),
            None
        );
        assert_eq!(
            warnings,
            vec![BackendWarning::UnknownExplicitBackend {
                backend: "not-a-backend".to_owned(),
            }]
        );
    }

    #[test]
    fn explicit_parakeet_below_floor_warns() {
        assert_eq!(
            warn_if_local_below_floor(Some("parakeet"), "parakeet", Some(1), Some(2)),
            Some(BackendWarning::ExplicitParakeetBelowFloor)
        );
    }

    #[test]
    fn explicit_parakeet_cpp_below_floor_does_not_warn() {
        assert_eq!(
            warn_if_local_below_floor(Some("parakeet-cpp"), "parakeet-cpp", Some(1), Some(2)),
            None
        );
    }

    #[test]
    fn unset_explicit_backend_below_floor_does_not_warn() {
        assert_eq!(
            warn_if_local_below_floor(None, "parakeet", Some(1), Some(2)),
            None
        );
    }

    #[test]
    fn stt_surface_is_a_hard_driver_error() {
        let error = resolve_default_backend(None, None, None, None, false, false).unwrap_err();

        assert!(matches!(error, TranscribeError::SttSurface { .. }));
        assert_eq!(error.exit_code(), 1);
    }

    #[test]
    fn resolution_delegates_to_core_choice_after_unknown_backend_downgrade() {
        let resolution = resolve_default_backend(
            Some("not-a-backend"),
            Some("parakeet"),
            Some(4 * GIB),
            Some(4 * GIB),
            false,
            false,
        )
        .unwrap();

        assert_eq!(resolution.backend, "parakeet");
        assert_eq!(
            resolution.warnings,
            vec![BackendWarning::UnknownExplicitBackend {
                backend: "not-a-backend".to_owned(),
            }]
        );
    }
}
