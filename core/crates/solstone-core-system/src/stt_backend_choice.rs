// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure resource-aware STT backend selection.
//!
//! A faithful port of `solstone.observe.transcribe.resource.resolve_stt_backend_choice`
//! (Python). Two in-scope supervisor call sites -- the admission latch and
//! `linux_stt_uses_parakeet_cpp` -- depend on this decision at runtime, so a
//! Rust supervisor that re-derives it independently would silently fork an
//! already-shared decision a fourth way.

pub const STT_SURFACE: &str = "surface";

/// Mirrors Python's `resolve_stt_backend_choice` field-for-field and
/// branch-for-branch. No config, environment, or machine state is read here
/// -- every input is a parameter, exactly like the Python original's own
/// contract ("Resolve STT backend choice without reading config, env, or
/// machine state").
pub fn resolve_stt_backend_choice(
    explicit_backend: Option<&str>,
    available_bytes: Option<u64>,
    floor_bytes: Option<u64>,
    local_backend: Option<&str>,
    confidential_lane_active: bool,
    confidential_audio_enabled: bool,
) -> String {
    if matches!(explicit_backend, Some("parakeet") | Some("parakeet-cpp")) {
        return explicit_backend.expect("checked Some above").to_owned();
    }
    if explicit_backend == Some("confidential") {
        if confidential_lane_active && confidential_audio_enabled {
            return "confidential".to_owned();
        }
        return local_backend_or_surface(local_backend);
    }

    if confidential_lane_active && confidential_audio_enabled {
        return "confidential".to_owned();
    }
    if confidential_lane_active {
        return local_backend_or_surface(local_backend);
    }

    let local_fits = local_backend.is_some()
        && floor_bytes.is_some()
        && available_bytes.is_some()
        && available_bytes.expect("checked Some above") >= floor_bytes.expect("checked Some above");
    if local_fits {
        return local_backend.expect("checked Some above").to_owned();
    }
    STT_SURFACE.to_owned()
}

fn local_backend_or_surface(local_backend: Option<&str>) -> String {
    local_backend
        .map(str::to_owned)
        .unwrap_or_else(|| STT_SURFACE.to_owned())
}

/// One row per backend-selection branch, named for the branch it exercises.
#[cfg(test)]
#[allow(clippy::type_complexity)]
fn decision_table() -> Vec<(
    &'static str,
    Option<&'static str>,
    Option<u64>,
    Option<u64>,
    Option<&'static str>,
    bool,
    bool,
    &'static str,
)> {
    vec![
        (
            "explicit_parakeet_wins_outright",
            Some("parakeet"),
            None,
            None,
            None,
            false,
            false,
            "parakeet",
        ),
        (
            "explicit_parakeet_cpp_wins_even_with_confidential_active",
            Some("parakeet-cpp"),
            None,
            None,
            Some("parakeet"),
            true,
            true,
            "parakeet-cpp",
        ),
        (
            "explicit_confidential_selected_when_lane_active_and_enabled",
            Some("confidential"),
            None,
            None,
            Some("parakeet"),
            true,
            true,
            "confidential",
        ),
        (
            "explicit_confidential_falls_back_to_local_when_audio_disabled",
            Some("confidential"),
            None,
            None,
            Some("parakeet"),
            true,
            false,
            "parakeet",
        ),
        (
            "explicit_confidential_falls_back_to_surface_when_no_local_backend",
            Some("confidential"),
            None,
            None,
            None,
            true,
            false,
            STT_SURFACE,
        ),
        (
            "explicit_confidential_falls_back_when_lane_inactive",
            Some("confidential"),
            None,
            None,
            Some("parakeet"),
            false,
            false,
            "parakeet",
        ),
        (
            "no_explicit_backend_confidential_lane_and_audio_enabled",
            None,
            None,
            None,
            Some("parakeet"),
            true,
            true,
            "confidential",
        ),
        (
            "no_explicit_backend_confidential_lane_active_audio_disabled_falls_back_to_local",
            None,
            Some(0),
            Some(u64::MAX),
            Some("parakeet"),
            true,
            false,
            "parakeet",
        ),
        (
            "no_explicit_backend_confidential_lane_active_audio_disabled_falls_back_to_surface",
            None,
            None,
            None,
            None,
            true,
            false,
            STT_SURFACE,
        ),
        (
            "local_fits_when_available_meets_floor",
            None,
            Some(8 * 1024 * 1024 * 1024),
            Some(4 * 1024 * 1024 * 1024),
            Some("parakeet"),
            false,
            false,
            "parakeet",
        ),
        (
            "local_fits_at_exact_floor_boundary",
            None,
            Some(4 * 1024 * 1024 * 1024),
            Some(4 * 1024 * 1024 * 1024),
            Some("parakeet"),
            false,
            false,
            "parakeet",
        ),
        (
            "local_does_not_fit_below_floor",
            None,
            Some(1),
            Some(4 * 1024 * 1024 * 1024),
            Some("parakeet"),
            false,
            false,
            STT_SURFACE,
        ),
        (
            "local_backend_absent_falls_back_to_surface",
            None,
            Some(8 * 1024 * 1024 * 1024),
            Some(4 * 1024 * 1024 * 1024),
            None,
            false,
            false,
            STT_SURFACE,
        ),
        (
            "floor_bytes_unknown_falls_back_to_surface",
            None,
            Some(8 * 1024 * 1024 * 1024),
            None,
            Some("parakeet"),
            false,
            false,
            STT_SURFACE,
        ),
        (
            "available_bytes_unknown_falls_back_to_surface",
            None,
            None,
            Some(4 * 1024 * 1024 * 1024),
            Some("parakeet"),
            false,
            false,
            STT_SURFACE,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_table_matches_expected_choice() {
        for (name, explicit, available, floor, local, lane_active, audio_enabled, expected) in
            decision_table()
        {
            assert_eq!(
                resolve_stt_backend_choice(
                    explicit,
                    available,
                    floor,
                    local,
                    lane_active,
                    audio_enabled
                ),
                expected,
                "{name}"
            );
        }
    }
}
