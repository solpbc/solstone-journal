// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sibling-helper invocation for the out-of-process CED (ambient
//! sound-tagging) engine.
//!
//! `solstone-core-ced-sys` `dlopen`s a dynamically-linked glibc shared object
//! (`libced.so`). A `musl-static`-lane process -- every binary that links
//! this crate -- has no in-process dynamic loader and can never satisfy that
//! call: see Brief D
//! (`vpe/workspace/archived/wave8-suze-owner-journal-burn-in-260831/brief-d-ced-out-of-process.md`),
//! measured on a shipped build. `solstone-core-ced-analyze` is the
//! `zig-gnu-2.27` sibling that owns `dlopen`ing CED instead; this module owns
//! resolving and invoking it.
//!
//! Mirrors [`super::super::vulkan`]'s `VulkanProbeProgram`/`resolve_program`
//! shape deliberately: the packaged sibling binary is the only production
//! program, and a test substitutes [`CedAnalyzeProgram::Explicit`] (e.g. a
//! tiny shell script) to exercise the JSON wire contract and error
//! classification without needing a compiled `solstone-core-ced-analyze`.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

const HELPER: &str = "solstone-core-ced-analyze";
/// The helper's `probe` argv token. Mirrors
/// `solstone_core_ced_analyze::PROBE_COMMAND`, which this crate cannot import:
/// `solstone-core-ced-analyze` owns `solstone-core-ced-sys` and therefore
/// `libloading`, and this crate is linked into `musl-static`-lane binaries that
/// must never reach a `dlopen` dependency (enforced by
/// `distribution_lane_dlopen_purity`). A bare invocation runs the helper in
/// CLASSIFY mode, which rejects a probe-schema request as `unknown-schema` --
/// the exact defect this constant exists to prevent.
pub const CED_PROBE_COMMAND: &str = "probe";
/// A classify request loads the engine and model once, then classifies every
/// window of one decoded audio file -- not a sub-second contract probe like
/// `solstone-core-transcribe::vad_runtime::VAD_RUNTIME_PROBE_TIMEOUT`, but it
/// must still never hang `journal check`, `install-models`, or a
/// transcription pass indefinitely if the helper wedges.
pub const CED_ANALYZE_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
static TEST_HELPER_BASE_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
#[cfg(test)]
static TEST_HELPER_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serializes tests that redirect [`CedAnalyzeProgram::SiblingHelper`]
/// resolution to a temporary directory, so parallel `cargo test` threads
/// cannot race the same process-wide override. Dropping the guard restores
/// production (current-executable-relative) resolution.
#[cfg(test)]
pub(crate) struct TestHelperGuard {
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestHelperGuard {
    fn drop(&mut self) {
        *TEST_HELPER_BASE_DIR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// Point [`CedAnalyzeProgram::SiblingHelper`] at `dir` for the guard's
/// lifetime, so a test can drop a stub named `solstone-core-ced-analyze`
/// there instead of needing a real compiled cross-lane binary.
#[cfg(test)]
pub(crate) fn set_test_helper_base_dir(dir: PathBuf) -> TestHelperGuard {
    let serial = TEST_HELPER_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *TEST_HELPER_BASE_DIR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(dir);
    TestHelperGuard { _serial: serial }
}

/// Program resolution for the isolated CED helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedAnalyzeProgram {
    /// Resolve the packaged sibling helper beside the current executable.
    SiblingHelper,
    /// Run an explicitly supplied child program for direct protocol tests.
    Explicit {
        executable: PathBuf,
        args: Vec<OsString>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedAnalyzeError {
    Unresolved {
        detail: String,
    },
    Spawn {
        detail: String,
    },
    Timeout,
    Io {
        detail: String,
    },
    /// Non-zero exit. `stderr`'s first line is typically a
    /// `solstone-ced-error-v1` JSON line the caller can parse further.
    Exit {
        code: Option<i32>,
        stderr: String,
    },
    MalformedResponse {
        detail: String,
    },
}

impl fmt::Display for CedAnalyzeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved { detail } => write!(formatter, "ced helper unresolved: {detail}"),
            Self::Spawn { detail } => write!(formatter, "ced helper could not start: {detail}"),
            Self::Timeout => write!(
                formatter,
                "ced helper did not exit within {CED_ANALYZE_TIMEOUT:?}"
            ),
            Self::Io { detail } => write!(formatter, "ced helper I/O failed: {detail}"),
            Self::Exit { code, stderr } => {
                write!(formatter, "ced helper exited {code:?}: {stderr}")
            }
            Self::MalformedResponse { detail } => {
                write!(
                    formatter,
                    "ced helper response was not valid JSON: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for CedAnalyzeError {}

struct ResolvedProgram {
    executable: PathBuf,
    args: Vec<OsString>,
}

fn resolve_program(program: &CedAnalyzeProgram) -> Result<ResolvedProgram, CedAnalyzeError> {
    match program {
        CedAnalyzeProgram::SiblingHelper => {
            #[cfg(test)]
            let base_dir = TEST_HELPER_BASE_DIR
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            #[cfg(not(test))]
            let base_dir: Option<PathBuf> = None;
            let base_dir = match base_dir {
                Some(dir) => dir,
                None => {
                    let current =
                        std::env::current_exe().map_err(|error| CedAnalyzeError::Unresolved {
                            detail: format!("could not determine current executable: {error}"),
                        })?;
                    current
                        .parent()
                        .ok_or_else(|| CedAnalyzeError::Unresolved {
                            detail: "current executable has no parent directory".to_owned(),
                        })?
                        .to_path_buf()
                }
            };
            let executable = base_dir.join(HELPER);
            let metadata =
                fs::metadata(&executable).map_err(|_error| CedAnalyzeError::Unresolved {
                    detail: format!("ced helper binary is missing: {}", executable.display()),
                })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return Err(CedAnalyzeError::Unresolved {
                        detail: format!(
                            "ced helper binary is not executable: {}",
                            executable.display()
                        ),
                    });
                }
            }
            #[cfg(not(unix))]
            let _ = metadata;
            Ok(ResolvedProgram {
                executable,
                args: Vec::new(),
            })
        }
        CedAnalyzeProgram::Explicit { executable, args } => Ok(ResolvedProgram {
            executable: executable.clone(),
            args: args.clone(),
        }),
    }
}

/// Send `request` to the CED helper (`probe` or bare classify) and parse a
/// single JSON response line from stdout.
pub fn invoke_ced_analyze(
    program: &CedAnalyzeProgram,
    request: &Value,
    timeout: Duration,
) -> Result<Value, CedAnalyzeError> {
    invoke_ced_analyze_with_args(program, &[], request, timeout)
}

/// [`invoke_ced_analyze`] with explicit leading argv tokens.
///
/// The helper dispatches on argv: bare is classify, `probe` is the readiness
/// probe. A probe-schema request sent to a bare invocation is rejected as
/// `unknown-schema`, so the probe caller MUST pass [`CED_PROBE_COMMAND`].
pub fn invoke_ced_analyze_with_args(
    program: &CedAnalyzeProgram,
    leading_args: &[&str],
    request: &Value,
    timeout: Duration,
) -> Result<Value, CedAnalyzeError> {
    let resolved = resolve_program(program)?;
    let mut child = Command::new(&resolved.executable)
        .args(leading_args.iter().map(OsString::from))
        .args(&resolved.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CedAnalyzeError::Spawn {
            detail: error.to_string(),
        })?;
    let body = request.to_string();
    child
        .stdin
        .take()
        .ok_or_else(|| CedAnalyzeError::Io {
            detail: "ced helper stdin was unavailable".to_owned(),
        })?
        .write_all(body.as_bytes())
        .map_err(|error| CedAnalyzeError::Io {
            detail: format!("could not write ced request: {error}"),
        })?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CedAnalyzeError::Timeout);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                return Err(CedAnalyzeError::Io {
                    detail: error.to_string(),
                });
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| CedAnalyzeError::Io {
            detail: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(CedAnalyzeError::Exit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|error| CedAnalyzeError::MalformedResponse {
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn write_stub(body: &str) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(HELPER);
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        (root, path)
    }

    fn explicit(executable: PathBuf) -> CedAnalyzeProgram {
        CedAnalyzeProgram::Explicit {
            executable,
            args: Vec::new(),
        }
    }

    #[test]
    fn sibling_helper_resolves_a_test_base_dir_and_reports_missing_otherwise() {
        let empty = tempfile::tempdir().unwrap();
        let _guard = set_test_helper_base_dir(empty.path().to_path_buf());
        let error = invoke_ced_analyze(
            &CedAnalyzeProgram::SiblingHelper,
            &Value::Null,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(
            matches!(error, CedAnalyzeError::Unresolved { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn sibling_helper_resolves_a_stub_dropped_at_the_test_base_dir() {
        let (root, _path) = write_stub(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-probe-response-v1\",\"ok\":true}'\n",
        );
        let _guard = set_test_helper_base_dir(root.path().to_path_buf());
        let response = invoke_ced_analyze(
            &CedAnalyzeProgram::SiblingHelper,
            &Value::Null,
            Duration::from_secs(2),
        )
        .expect("resolved sibling helper responds");
        assert_eq!(response["ok"], Value::Bool(true));
    }

    #[test]
    fn happy_path_stub_echoes_a_valid_response() {
        let (_root, path) = write_stub(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-probe-response-v1\",\"ok\":true}'\n",
        );
        let response = invoke_ced_analyze(&explicit(path), &Value::Null, Duration::from_secs(2))
            .expect("stub response parses");
        assert_eq!(response["ok"], Value::Bool(true));
    }

    /// W8-14 regression. The helper dispatches on argv: bare is CLASSIFY,
    /// `probe` is the readiness probe, and a probe-schema request sent to a
    /// bare invocation is rejected as `unknown-schema`. The readiness probe
    /// shipped without the token, so a correct, loadable engine reported
    /// `Unloadable` on every real install. The existing helper oracle could
    /// not catch it because it invokes the helper directly with the token
    /// rather than through this call path.
    #[test]
    fn leading_args_reach_the_helper_argv() {
        // The stub is the oracle: it succeeds only when argv[1] is `probe`,
        // exactly as the real helper's classify/probe dispatch behaves.
        let (_root, path) = write_stub(
            "#!/bin/sh\ncat >/dev/null\nif [ \"$1\" = \"probe\" ]; then \
printf '%s\\n' '{\"schema\":\"solstone-ced-probe-response-v1\",\"ok\":true}'; exit 0; fi\n\
printf '%s\\n' '{\"schema\":\"solstone-ced-error-v1\",\"reason\":\"unknown-schema\",\"detail\":\"bare invocation\"}' >&2\nexit 64\n",
        );
        let with_token = invoke_ced_analyze_with_args(
            &explicit(path.clone()),
            &[CED_PROBE_COMMAND],
            &Value::Null,
            Duration::from_secs(5),
        )
        .expect("probe token must reach the helper");
        assert_eq!(with_token.get("ok"), Some(&Value::Bool(true)));

        // Control: the same stub, same request, no token -> the failure the
        // founder's machine actually reported.
        let without = invoke_ced_analyze(&explicit(path), &Value::Null, Duration::from_secs(5))
            .expect_err("a bare invocation must not satisfy the probe");
        match without {
            CedAnalyzeError::Exit { code, stderr } => {
                assert_eq!(code, Some(64));
                assert!(stderr.contains("unknown-schema"), "{stderr}");
            }
            other => panic!("expected Exit, got {other:?}"),
        }
    }

    #[test]
    fn nonzero_exit_carries_stderr() {
        let (_root, path) = write_stub(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"schema\":\"solstone-ced-error-v1\",\"reason\":\"library-unloadable\",\"detail\":\"boom\"}' >&2\nexit 69\n",
        );
        let error =
            invoke_ced_analyze(&explicit(path), &Value::Null, Duration::from_secs(2)).unwrap_err();
        match error {
            CedAnalyzeError::Exit { code, stderr } => {
                assert_eq!(code, Some(69));
                assert!(stderr.contains("library-unloadable"), "{stderr}");
            }
            other => panic!("expected Exit, got {other:?}"),
        }
    }

    #[test]
    fn malformed_stdout_is_malformed_response() {
        let (_root, path) = write_stub("#!/bin/sh\ncat >/dev/null\nprintf 'not json'\n");
        // The subject here is response *parsing*, not timing. A tight wall-clock made
        // this flaky: under parallel test load, spawning the shell stub alone could
        // exceed the budget and the call returned `Timeout` instead of the malformed
        // response it exists to check. `timeout_kills_the_helper` covers the deadline.
        let error =
            invoke_ced_analyze(&explicit(path), &Value::Null, Duration::from_secs(60)).unwrap_err();
        assert!(matches!(error, CedAnalyzeError::MalformedResponse { .. }));
    }

    #[test]
    fn timeout_kills_the_helper() {
        let (_root, path) = write_stub("#!/bin/sh\ncat >/dev/null\nsleep 30\n");
        let error = invoke_ced_analyze(&explicit(path), &Value::Null, Duration::from_millis(100))
            .unwrap_err();
        assert!(matches!(error, CedAnalyzeError::Timeout));
    }

    #[test]
    fn missing_explicit_binary_is_a_spawn_error() {
        let error = invoke_ced_analyze(
            &explicit(PathBuf::from("/no/such/solstone-core-ced-analyze")),
            &Value::Null,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(matches!(error, CedAnalyzeError::Spawn { .. }));
    }
}
