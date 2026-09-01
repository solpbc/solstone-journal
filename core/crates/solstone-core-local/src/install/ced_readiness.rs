// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host verdict for the pinned ced.cpp sound-tagging assets.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ced_install::{
    ced_artifact_key, ced_library_path, ced_model_path, check_ced_assets, model_artifact,
};
use super::ced_runtime::{
    CED_ANALYZE_TIMEOUT, CedAnalyzeError, CedAnalyzeProgram, invoke_ced_analyze,
};
use super::manifest::sha256_file;

const PROBE_REQUEST_SCHEMA: &str = "solstone-ced-probe-request-v1";
const HELPER_ERROR_SCHEMA: &str = "solstone-ced-error-v1";

/// Owner-facing sentence for a degraded CED verdict, identical on every surface.
pub const CED_UNAVAILABLE_GUIDANCE: &str = "Sound tagging is degraded because its CED assets are unavailable. Transcription will continue. Use `journal install-models` to check or repair the CED assets.";

/// Short ready detail for `journal check` and `journal health`.
pub const CED_READY_DETAIL: &str = "ced.cpp sound-tag engine and model are ready";

/// Why a supported host is not Ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CedDegradedCause {
    Absent,
    IntegrityInvalid,
    Unloadable,
}

/// Result of probing CED assets on a host.
///
/// `os` and `arch` must already be canonical (`linux`/`x86_64`, `linux`/`arm64`,
/// `darwin`/`arm64`). Callers canonicalize at their own boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedReadiness {
    Ready {
        library: PathBuf,
        model: PathBuf,
    },
    Unsupported {
        os: String,
        arch: String,
    },
    Degraded {
        cause: CedDegradedCause,
        detail: String,
    },
}

/// Production verdict: catalog model digest, then [`evaluate_ced_readiness_against`].
///
/// `os` and `arch` must already be canonical.
pub fn evaluate_ced_readiness(journal: &Path, os: &str, arch: &str) -> CedReadiness {
    match model_artifact() {
        Ok(artifact) => evaluate_ced_readiness_against(journal, os, arch, artifact.sha256),
        Err(error) => CedReadiness::Degraded {
            cause: CedDegradedCause::IntegrityInvalid,
            detail: error.to_string(),
        },
    }
}

/// Verdict against an explicit model digest.
///
/// Production [`evaluate_ced_readiness`] supplies the catalog sha256. Tests
/// supply a digest that matches a fixture so the load probe can run.
///
/// `os` and `arch` must already be canonical.
pub fn evaluate_ced_readiness_against(
    journal: &Path,
    os: &str,
    arch: &str,
    expected_model_sha256: &str,
) -> CedReadiness {
    evaluate_ced_readiness_against_with_probe(
        journal,
        os,
        arch,
        expected_model_sha256,
        |library, model| probe_ced_engine(&CedAnalyzeProgram::SiblingHelper, library, model),
    )
}

/// [`evaluate_ced_readiness_against`] with the deep engine probe supplied by
/// the caller.
///
/// Production always passes the real out-of-process probe (Brief D:
/// `solstone-core-ced-sys` `dlopen`s a glibc shared object that this
/// `musl-static`-lane crate can never load in-process). Callers in other
/// crates' tests -- `solstone-core::install_models`,
/// `solstone-core-sound-tags` -- substitute a closure to get a deterministic
/// `Ready`/`Unloadable` verdict without a compiled cross-lane helper or a
/// native shared-library fixture, exactly the seam
/// `solstone-core::install_models`'s own `hooks.ced_verdict` function
/// pointer already uses one level up.
pub fn evaluate_ced_readiness_against_with_probe(
    journal: &Path,
    os: &str,
    arch: &str,
    expected_model_sha256: &str,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedReadiness {
    let Some(key) = ced_artifact_key(os, arch) else {
        return CedReadiness::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        };
    };
    match check_ced_assets(journal, os, arch) {
        Ok(None) => CedReadiness::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        },
        Err(error) => {
            let cause = match error.reason_code.as_str() {
                "sidecar_missing" | "file_missing" => CedDegradedCause::Absent,
                _ => CedDegradedCause::IntegrityInvalid,
            };
            CedReadiness::Degraded {
                cause,
                detail: error.to_string(),
            }
        }
        Ok(Some(_)) => probe_integrity_and_load(journal, key, expected_model_sha256, probe),
    }
}

fn probe_integrity_and_load(
    journal: &Path,
    key: &str,
    expected_model_sha256: &str,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedReadiness {
    let model = ced_model_path(journal);
    let actual = match sha256_file(&model) {
        Ok(actual) => actual,
        Err(detail) => {
            return CedReadiness::Degraded {
                cause: CedDegradedCause::IntegrityInvalid,
                detail,
            };
        }
    };
    if actual != expected_model_sha256 {
        return CedReadiness::Degraded {
            cause: CedDegradedCause::IntegrityInvalid,
            detail: format!(
                "sha256 mismatch for {}: expected {expected_model_sha256}, got {actual}",
                model.display()
            ),
        };
    }
    let library = ced_library_path(journal, key);
    match probe(&library, &model) {
        Ok(()) => CedReadiness::Ready { library, model },
        Err(detail) => CedReadiness::Degraded {
            cause: CedDegradedCause::Unloadable,
            detail,
        },
    }
}

/// Ask the out-of-process `solstone-core-ced-analyze` helper (Brief D) to
/// open `library` and load `model`, without decoding or classifying any
/// audio. This is exactly the pair of calls `CedLibrary::open` +
/// `load_model` used to make in-process before this crate could never
/// satisfy them from a `musl-static` binary.
fn probe_ced_engine(
    program: &CedAnalyzeProgram,
    library: &Path,
    model: &Path,
) -> Result<(), String> {
    let request = json!({
        "schema": PROBE_REQUEST_SCHEMA,
        "models": {
            "ced_library_path": library,
            "ced_model_path": model,
        },
    });
    match invoke_ced_analyze(program, &request, CED_ANALYZE_TIMEOUT) {
        Ok(response) if response.get("ok") == Some(&Value::Bool(true)) => Ok(()),
        Ok(response) => Err(format!(
            "ced probe helper returned an unexpected response: {response}"
        )),
        Err(CedAnalyzeError::Exit { stderr, code }) => Err(helper_error_detail(&stderr)
            .unwrap_or_else(|| format!("ced probe helper exited {code:?}: {stderr}"))),
        Err(error) => Err(error.to_string()),
    }
}

/// Pull `detail` out of the helper's `solstone-ced-error-v1` stderr line, if
/// present, so the readiness detail reads like the direct `CedError` message
/// this replaced rather than a generic process-exit summary.
fn helper_error_detail(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        (value.get("schema").and_then(Value::as_str) == Some(HELPER_ERROR_SCHEMA))
            .then(|| {
                value
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::install::archive::DownloadHostPolicy;
    use crate::install::ced_fixture::{write_ced_model_bytes, write_complete_ced_install};
    use crate::install::ced_install::{
        ced_library_path, ced_model_path, check_ced_assets, install_ced_assets_with_policy,
        model_artifact,
    };

    /// Refuses every host, so any attempted download surfaces as a hard
    /// error instead of silently succeeding against the real network -- the
    /// fixture stays hermetic and any actual install attempt is unmissable.
    const DENY_ALL_POLICY: DownloadHostPolicy<'static> = DownloadHostPolicy {
        allowed_hosts: &["example.invalid"],
        allow_http: false,
        origin_base_url: "https://updates.solstone.app",
    };

    #[test]
    fn unsupported_platform_is_unsupported() {
        let journal = tempfile::tempdir().unwrap();
        match evaluate_ced_readiness(journal.path(), "windows", "x86_64") {
            CedReadiness::Unsupported { os, arch } => {
                assert_eq!(os, "windows");
                assert_eq!(arch, "x86_64");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
        match evaluate_ced_readiness(journal.path(), "macos", "aarch64") {
            CedReadiness::Unsupported { os, arch } => {
                assert_eq!(os, "macos");
                assert_eq!(arch, "aarch64");
            }
            other => panic!("expected unsupported raw macos, got {other:?}"),
        }
    }

    #[test]
    fn absent_sidecar_is_absent() {
        let journal = tempfile::tempdir().unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedReadiness::Degraded {
                cause: CedDegradedCause::Absent,
                ..
            } => {}
            other => panic!("expected absent, got {other:?}"),
        }
    }

    #[test]
    fn size_ok_but_digest_mismatch_is_integrity_invalid() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedReadiness::Degraded {
                cause: CedDegradedCause::IntegrityInvalid,
                ..
            } => {}
            other => panic!("expected integrity-invalid, got {other:?}"),
        }
    }

    /// Hermetic reproduction of Brief A's actual failure mode: a journal
    /// root where the sidecar, size and nonempty-file checks all pass --
    /// exactly what `check_ced_assets` (the installer's own idempotency
    /// gate) verifies -- while the model's real bytes still fail the
    /// deeper, catalog-digest-based readiness verdict this module computes.
    /// `write_complete_ced_install` writes a model padded to the catalog's
    /// `size_bytes` but never fills it with the catalog's actual bytes, so
    /// it is size-correct and digest-wrong: precisely the class of
    /// degradation `check_ced_assets`'s "deliberately weak" comment
    /// (ced_install.rs) says it cannot see.
    ///
    /// This shows the mechanism behind "why does the repair command fail
    /// against it": `install_ced_assets_with_policy(force: false)` reads
    /// the weak gate as "already fine" and returns the untouched, still-
    /// broken record without attempting any download -- proven here via a
    /// host-refusing policy, so any actual attempt would be unmissable.
    #[test]
    fn shallow_check_passes_while_deep_readiness_disagrees_and_repair_is_a_noop() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();

        // The weak gate: sidecar/size/nonempty all check out.
        assert!(
            check_ced_assets(journal.path(), "linux", "x86_64").is_ok(),
            "fixture must pass the installer's own shallow idempotency check"
        );
        // The strong gate (what the caller actually used to decide a repair
        // was needed): the model's real bytes are all zero, not the
        // catalog's pinned content, so the deep verdict disagrees.
        assert!(
            matches!(
                evaluate_ced_readiness(journal.path(), "linux", "x86_64"),
                CedReadiness::Degraded {
                    cause: CedDegradedCause::IntegrityInvalid,
                    ..
                }
            ),
            "fixture must fail the deep, catalog-digest readiness verdict"
        );

        let before = fs::read(ced_model_path(journal.path())).unwrap();
        let result = install_ced_assets_with_policy(
            journal.path(),
            "linux",
            "x86_64",
            false,
            &DENY_ALL_POLICY,
        );
        assert!(
            result.is_ok(),
            "the weak gate reports success, so the installer skips reinstalling: {result:?}"
        );
        let after = fs::read(ced_model_path(journal.path())).unwrap();
        assert_eq!(
            before, after,
            "no repair was attempted -- the deny-all policy never even had a chance to refuse a download"
        );
    }

    /// A failing deep probe -- whatever the underlying reason, garbage
    /// engine bytes, a wrong ABI, a missing symbol, or (as in this test) the
    /// out-of-process helper simply refusing -- must still land on
    /// `Unloadable` once assets are digest-valid. The real dlopen-level
    /// distinctions (garbage bytes, ABI mismatch, missing symbol) are
    /// covered where they can actually be observed:
    /// `solstone-core-ced-analyze`'s own tests, which dlopen a real compiled
    /// stub. This test proves the wiring around that outcome, not the
    /// native failure itself.
    #[test]
    fn against_digest_allows_unloadable_garbage_library() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        let digest = sha256_file(&ced_model_path(journal.path())).unwrap();
        match evaluate_ced_readiness_against_with_probe(
            journal.path(),
            "linux",
            "x86_64",
            &digest,
            |_library, _model| Err("stub: engine refused to load".to_owned()),
        ) {
            CedReadiness::Degraded {
                cause: CedDegradedCause::Unloadable,
                detail,
            } => {
                assert!(detail.contains("stub: engine refused to load"), "{detail}");
            }
            other => panic!("expected unloadable, got {other:?}"),
        }
    }

    #[test]
    fn production_wrapper_reads_catalog() {
        let expected = model_artifact().unwrap().sha256;
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        write_ced_model_bytes(journal.path(), b"not-the-pin").unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedReadiness::Degraded {
                cause: CedDegradedCause::IntegrityInvalid,
                detail,
            } => {
                assert!(detail.contains(expected), "{detail}");
            }
            other => panic!("expected catalog-digest mismatch, got {other:?}"),
        }
    }

    #[test]
    fn ready_against_its_own_digest_when_the_probe_succeeds() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        let library = ced_library_path(journal.path(), "linux-cpu-x64");
        let digest = sha256_file(&ced_model_path(journal.path())).unwrap();
        match evaluate_ced_readiness_against_with_probe(
            journal.path(),
            "linux",
            "x86_64",
            &digest,
            |probed_library, probed_model| {
                assert_eq!(probed_library, library);
                assert_eq!(probed_model, ced_model_path(journal.path()));
                Ok(())
            },
        ) {
            CedReadiness::Ready { .. } => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }

    /// End-to-end through the real, unparameterized
    /// [`evaluate_ced_readiness_against`] and the real out-of-process
    /// plumbing (`ced_runtime::invoke_ced_analyze`,
    /// `CedAnalyzeProgram::SiblingHelper`) -- not the `_with_probe` seam
    /// above. The stub stands in for the compiled `solstone-core-ced-analyze`
    /// binary (a genuine cross-compiled zig-gnu-2.27 binary is not buildable
    /// from a dev `cargo test` run); the process boundary, argv/JSON
    /// contract, and exit-code classification it exercises are real.
    #[test]
    fn real_subprocess_probe_drives_the_unparameterized_readiness_functions() {
        use crate::install::ced_runtime::set_test_helper_base_dir;

        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        let digest = sha256_file(&ced_model_path(journal.path())).unwrap();

        let helpers = tempfile::tempdir().unwrap();
        let helper_path = helpers.path().join("solstone-core-ced-analyze");
        fs::write(
            &helper_path,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-probe-response-v1\",\"ok\":true}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _guard = set_test_helper_base_dir(helpers.path().to_path_buf());
        match evaluate_ced_readiness_against(journal.path(), "linux", "x86_64", &digest) {
            CedReadiness::Ready { .. } => {}
            other => panic!("expected ready via the real subprocess stub, got {other:?}"),
        }

        fs::write(
            &helper_path,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-error-v1\",\"reason\":\"library-unloadable\",\"detail\":\"stub refuses\"}' >&2\nexit 69\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&helper_path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        match evaluate_ced_readiness_against(journal.path(), "linux", "x86_64", &digest) {
            CedReadiness::Degraded {
                cause: CedDegradedCause::Unloadable,
                detail,
            } => {
                assert!(detail.contains("stub refuses"), "{detail}");
            }
            other => panic!("expected unloadable via the real subprocess stub, got {other:?}"),
        }
    }
}
