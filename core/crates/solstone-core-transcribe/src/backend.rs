// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-input collection around the shared pure STT backend choice.

pub(crate) mod confidential;
#[cfg(unix)]
pub(crate) mod parakeet_coreml;
pub(crate) mod parakeet_cpp;

use std::env;
use std::fs;

use solstone_core_assets::{Platform, resolve_host_platform};
use solstone_core_system::stt_backend_choice::{STT_SURFACE, resolve_stt_backend_choice};

use crate::TranscribeError;

const GIB: u64 = 1024 * 1024 * 1024;
const LINUX_LOCAL_FLOOR_BYTES: u64 = 4 * GIB;
const DARWIN_ARM64_LOCAL_FLOOR_BYTES: u64 = 2 * GIB;
pub(crate) const KNOWN_BACKENDS: [&str; 3] = ["parakeet", "parakeet-cpp", "confidential"];

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

/// Read currently available RAM when this host exposes the Linux meminfo view.
pub(crate) fn read_available_bytes() -> Option<u64> {
    if env::consts::OS != "linux" {
        return None;
    }
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    parse_available_bytes(&meminfo)
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
    match resolve_host_platform(os, arch).ok()? {
        Platform::MacosArm64 => Some(DARWIN_ARM64_LOCAL_FLOOR_BYTES),
        Platform::LinuxX64 | Platform::LinuxArm64 => Some(LINUX_LOCAL_FLOOR_BYTES),
    }
}

fn local_stt_backend_for(os: &str, arch: &str) -> Option<&'static str> {
    platform_floor_bytes_for(os, arch).map(|_| "parakeet")
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

fn parse_available_bytes(meminfo: &str) -> Option<u64> {
    let available = meminfo_value_kib(meminfo, "MemAvailable")?;
    let total = meminfo_value_kib(meminfo, "MemTotal")?;
    if available == 0 || total == 0 || available > total {
        return None;
    }
    available.checked_mul(1024)
}

fn meminfo_value_kib(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let (found_key, value) = line.split_once(':')?;
        if found_key != key {
            return None;
        }
        let mut parts = value.split_whitespace();
        let kib = parts.next()?.parse().ok()?;
        matches!(parts.next(), Some("kB")).then_some(kib)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BackendWarning, DARWIN_ARM64_LOCAL_FLOOR_BYTES, LINUX_LOCAL_FLOOR_BYTES,
        local_stt_backend_for, normalize_explicit_backend, parse_available_bytes,
        platform_floor_bytes_for, resolve_default_backend, warn_if_local_below_floor,
    };
    use crate::TranscribeError;

    #[test]
    fn linux_platform_floor_is_four_gib() {
        assert_eq!(
            platform_floor_bytes_for("linux", "x86_64"),
            Some(LINUX_LOCAL_FLOOR_BYTES)
        );
    }

    #[test]
    fn darwin_arm64_platform_floor_is_two_gib() {
        assert_eq!(
            platform_floor_bytes_for("darwin", "arm64"),
            Some(DARWIN_ARM64_LOCAL_FLOOR_BYTES)
        );
    }

    #[test]
    fn macos_aarch64_platform_floor_is_two_gib() {
        assert_eq!(
            platform_floor_bytes_for("macos", "aarch64"),
            Some(DARWIN_ARM64_LOCAL_FLOOR_BYTES)
        );
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
        assert_eq!(local_stt_backend_for("windows", "x86_64"), None);
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
            Some(LINUX_LOCAL_FLOOR_BYTES),
            Some(LINUX_LOCAL_FLOOR_BYTES),
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

    #[test]
    fn meminfo_available_bytes_require_valid_available_and_total() {
        assert_eq!(
            parse_available_bytes("MemTotal: 2048 kB\nMemAvailable: 1024 kB\n"),
            Some(1024 * 1024)
        );
        assert_eq!(
            parse_available_bytes("MemTotal: 1024 kB\nMemAvailable: 2048 kB\n"),
            None
        );
    }
}
