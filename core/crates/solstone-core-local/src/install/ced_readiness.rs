// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host verdict for the pinned ced.cpp sound-tagging assets.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
use solstone_core_distribution::windows_payload::{
    VerifiedWindowsPayload, WINDOWS_CED_LIBRARY, verify_windows_payload,
};

use serde_json::{Value, json};

use super::capability_status::CapabilityStatus;
use super::ced_install::{
    ced_artifact_key, ced_library_path, ced_model_path, ced_uses_package_engine, check_ced_assets,
    check_ced_model, model_artifact,
};
use super::ced_runtime::{
    CED_ANALYZE_TIMEOUT, CED_PROBE_COMMAND, CedAnalyzeError, CedAnalyzeProgram,
    invoke_ced_analyze_with_args,
};
use super::manifest::sha256_file;

/// Owner-facing sentence for a degraded CED verdict, identical on every surface.
pub const CED_UNAVAILABLE_GUIDANCE: &str = "Sound tagging is degraded because its CED assets are unavailable. Transcription will continue. Use `journal install-models` to check or repair the CED assets. If the signed CED app payload is unavailable on Windows, reinstall the journal app.";

/// Short ready detail for `journal check` and `journal health`.
pub const CED_READY_DETAIL: &str = "ced.cpp sound-tag engine and model are ready";

/// Capability identifier carried on every CED-constructed non-ready status.
pub const CED_CAPABILITY: &str = "ced";

/// Result of probing CED assets on a host.
///
/// `os` and `arch` must already be canonical (`linux`/`x86_64`, `linux`/`arm64`,
/// `darwin`/`arm64`). Callers canonicalize at their own boundary.
///
/// `Degraded` is never constructed with [`CapabilityStatus::Ready`]; a ready
/// host is [`CedVerdict::Ready`] directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedVerdict {
    Ready { library: PathBuf, model: PathBuf },
    Unsupported { os: String, arch: String },
    Degraded(CapabilityStatus),
}

/// Production verdict: catalog model digest, then [`evaluate_ced_readiness_against`].
///
/// `os` and `arch` must already be canonical.
pub fn evaluate_ced_readiness(journal: &Path, os: &str, arch: &str) -> CedVerdict {
    match model_artifact() {
        Ok(artifact) => evaluate_ced_readiness_against(journal, os, arch, artifact.sha256),
        Err(error) => CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid {
            capability: CED_CAPABILITY.to_owned(),
            detail: error.to_string(),
        }),
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
) -> CedVerdict {
    evaluate_ced_readiness_against_with_probe(
        journal,
        os,
        arch,
        expected_model_sha256,
        |library, model| probe_ced_engine(&CedAnalyzeProgram::SiblingHelper, library, model),
    )
}

/// [`evaluate_ced_readiness_against`] with the deep engine probe supplied by the caller.
///
/// 🔴 Production always passes the real **out-of-process** probe. `solstone-core-ced-sys`
/// `dlopen`s a glibc shared object, and this crate ships on the `musl-static` lane, which
/// has no dynamic loader -- an in-process load can never succeed in a shipped build. Tests
/// in sibling crates substitute a closure to get a deterministic verdict without a compiled
/// cross-lane helper.
pub fn evaluate_ced_readiness_against_with_probe(
    journal: &Path,
    os: &str,
    arch: &str,
    expected_model_sha256: &str,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedVerdict {
    if ced_uses_package_engine(os, arch) {
        return probe_windows_package_engine(journal, expected_model_sha256, probe);
    }
    let Some(key) = ced_artifact_key(os, arch) else {
        return CedVerdict::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        };
    };
    match check_ced_assets(journal, os, arch) {
        Ok(None) => CedVerdict::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        },
        Err(error) => CedVerdict::Degraded(ced_install_status(&error)),
        Ok(Some(_)) => probe_integrity_and_load(journal, key, expected_model_sha256, probe),
    }
}

fn probe_windows_package_engine(
    journal: &Path,
    expected_model_sha256: &str,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedVerdict {
    let library = match windows_package_ced_library() {
        Ok(library) => library,
        Err(detail) => {
            return CedVerdict::Degraded(CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            });
        }
    };
    if let Err(error) = check_ced_model(journal) {
        return CedVerdict::Degraded(ced_install_status(&error));
    }
    let model = ced_model_path(journal);
    probe_model_and_library_at_paths(&model, expected_model_sha256, &library, probe)
}

#[cfg(windows)]
fn windows_package_ced_library() -> Result<PathBuf, String> {
    static PAYLOAD: OnceLock<Result<VerifiedWindowsPayload, String>> = OnceLock::new();
    let payload = PAYLOAD.get_or_init(|| {
        let executable = std::env::current_exe().map_err(|error| {
            format!("could not determine the running journal executable: {error}")
        })?;
        let bin = executable.parent().ok_or_else(|| {
            format!(
                "running journal executable has no containing directory: {}",
                executable.display()
            )
        })?;
        if bin.file_name() != Some(OsStr::new("bin")) {
            return Err(format!(
                "running journal executable is not in the package bin directory: {}",
                executable.display()
            ));
        }
        let root = bin.parent().ok_or_else(|| {
            format!(
                "package bin directory has no package root: {}",
                bin.display()
            )
        })?;
        verify_windows_payload(root)
            .map_err(|error| format!("could not verify the signed CED app payload: {error}"))
    });
    payload
        .as_ref()
        .map_err(Clone::clone)?
        .ced_library_path()
        .map_err(|error| {
            format!("signed CED app payload does not declare {WINDOWS_CED_LIBRARY}: {error}")
        })
}

#[cfg(not(windows))]
fn windows_package_ced_library() -> Result<PathBuf, String> {
    Err("Windows CED package verification requires a Windows runtime".to_owned())
}

fn ced_install_status(error: &super::ced_install::CedInstallError) -> CapabilityStatus {
    match error.reason_code.as_str() {
        "sidecar_missing" | "file_missing" => CapabilityStatus::Absent {
            capability: CED_CAPABILITY.to_owned(),
            detail: error.to_string(),
        },
        _ => CapabilityStatus::IntegrityInvalid {
            capability: CED_CAPABILITY.to_owned(),
            detail: error.to_string(),
        },
    }
}

fn probe_integrity_and_load(
    journal: &Path,
    key: &str,
    expected_model_sha256: &str,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedVerdict {
    let model = ced_model_path(journal);
    let library = ced_library_path(journal, key);
    probe_model_and_library_at_paths(&model, expected_model_sha256, &library, probe)
}

fn probe_model_and_library_at_paths(
    model: &Path,
    expected_model_sha256: &str,
    library: &Path,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedVerdict {
    let actual = match sha256_file(model) {
        Ok(actual) => actual,
        Err(detail) => {
            return CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            });
        }
    };
    if actual != expected_model_sha256 {
        return CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid {
            capability: CED_CAPABILITY.to_owned(),
            detail: format!(
                "sha256 mismatch for {}: expected {expected_model_sha256}, got {actual}",
                model.display()
            ),
        });
    }
    if let Err(detail) = probe(library, model) {
        return CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable {
            capability: CED_CAPABILITY.to_owned(),
            detail,
        });
    }
    CedVerdict::Ready {
        library: library.to_path_buf(),
        model: model.to_path_buf(),
    }
}

const PROBE_REQUEST_SCHEMA: &str = "solstone-ced-probe-request-v1";
const HELPER_ERROR_SCHEMA: &str = "solstone-ced-error-v1";

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
    match invoke_ced_analyze_with_args(program, &[CED_PROBE_COMMAND], &request, CED_ANALYZE_TIMEOUT)
    {
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
    #[test]
    fn windows_requires_a_verified_package_engine() {
        let journal = tempfile::tempdir().unwrap();
        match evaluate_ced_readiness(journal.path(), "windows", "x86_64") {
            CedVerdict::Degraded(CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                detail,
                ..
            }) => {}
            other => panic!("expected package-engine refusal, got {other:?}"),
        }
        match evaluate_ced_readiness(journal.path(), "macos", "aarch64") {
            CedVerdict::Unsupported { os, arch } => {
                assert_eq!(os, "macos");
                assert_eq!(arch, "aarch64");
            }
            other => panic!("expected unsupported raw macos, got {other:?}"),
        }
    }

    use super::*;

    /// W8-14 regression, pinned at the exact call site that shipped broken.
    ///
    /// `probe_ced_engine` builds a PROBE-schema request. The helper dispatches
    /// on argv -- bare is CLASSIFY -- so omitting the `probe` token makes the
    /// helper reject a perfectly good engine as `unknown-schema`, which this
    /// module then reports as `Unloadable`. That is precisely what the
    /// founder's machine did: the helper answered `{"ok":true}` when invoked by
    /// hand with the token, while `journal check` reported `unloadable`.
    ///
    /// The sibling tests below inject a probe CLOSURE and therefore cannot see
    /// this class at all; this one drives the real invocation path.
    #[test]
    fn probe_ced_engine_invokes_the_helper_in_probe_mode() {
        let root = tempfile::tempdir().expect("temp root");
        let stub = root.path().join("helper.sh");
        std::fs::write(
            &stub,
            "#!/bin/sh\ncat >/dev/null\nif [ \"$1\" = \"probe\" ]; then \
printf '%s\\n' '{\"schema\":\"solstone-ced-probe-response-v1\",\"ok\":true}'; exit 0; fi\n\
printf '%s\\n' '{\"schema\":\"solstone-ced-error-v1\",\"reason\":\"unknown-schema\",\"detail\":\"bare invocation is classify\"}' >&2\nexit 64\n",
        )
        .expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }
        let program = CedAnalyzeProgram::Explicit {
            executable: stub,
            args: Vec::new(),
        };
        let library = root.path().join("libced.so");
        let model = root.path().join("model.gguf");
        probe_ced_engine(&program, &library, &model)
            .expect("the readiness probe must invoke the helper in probe mode");
    }
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
        // Windows is no longer "unsupported": it resolves through the signed
        // package engine, which cannot be verified off a Windows runtime. That
        // path has its own test (`windows_requires_a_verified_package_engine`).
        match evaluate_ced_readiness(journal.path(), "freebsd", "x86_64") {
            CedVerdict::Unsupported { os, arch } => {
                assert_eq!(os, "freebsd");
                assert_eq!(arch, "x86_64");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
        match evaluate_ced_readiness(journal.path(), "macos", "aarch64") {
            CedVerdict::Unsupported { os, arch } => {
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
            CedVerdict::Degraded(CapabilityStatus::Absent { detail, .. }) => {}
            other => panic!("expected absent, got {other:?}"),
        }
    }

    #[test]
    fn size_ok_but_digest_mismatch_is_integrity_invalid() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid { detail, .. }) => {}
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
                CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid { .. })
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
            CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable { detail, .. }) => {
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
            CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid { detail, .. }) => {
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
            CedVerdict::Ready { .. } => {}
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
            CedVerdict::Ready { .. } => {}
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
            CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable { detail, .. }) => {
                assert!(detail.contains("stub refuses"), "{detail}");
            }
            other => panic!("expected unloadable via the real subprocess stub, got {other:?}"),
        }
    }
}
