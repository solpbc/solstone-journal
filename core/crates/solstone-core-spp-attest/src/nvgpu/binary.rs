// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! nvattest binary path and command construction.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{error::GpuAppraisalReason, tlv::SPDM_NONCE_SIZE};

const NVATTEST_LIB_RELPATH: &str = "lib";
const CA_BUNDLE_RELATIVE_PATH: &str = "share/ca/ca-bundle.pem";

/// Resolved files required to invoke an nvattest payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvattestInstallation {
    pub binary: PathBuf,
    pub lib_dir: PathBuf,
    pub ca_bundle: PathBuf,
}

/// Shell-free nvattest invocation arguments and environment overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvattestCommand {
    pub executable: PathBuf,
    /// Includes `executable` as argv[0], matching Python's subprocess argv.
    pub argv: Vec<OsString>,
    /// Environment additions; the process otherwise inherits its environment.
    pub env: BTreeMap<OsString, OsString>,
}

/// Resolves and validates the nvattest payload beneath `nvattest_dir`.
pub fn locate_nvattest(nvattest_dir: &Path) -> Result<NvattestInstallation, GpuAppraisalReason> {
    if !nvattest_dir.is_dir() {
        return Err(GpuAppraisalReason::NvattestUnavailable);
    }
    let root =
        std::fs::canonicalize(nvattest_dir).map_err(|_| GpuAppraisalReason::NvattestUnavailable)?;
    let binary = root.join("bin/nvattest");
    let lib_dir = root.join(NVATTEST_LIB_RELPATH);
    let ca_bundle = root.join(CA_BUNDLE_RELATIVE_PATH);

    if !binary.is_file() || !lib_dir.is_dir() {
        return Err(GpuAppraisalReason::NvattestUnavailable);
    }
    if !ca_bundle.is_file() {
        return Err(GpuAppraisalReason::NvattestIntegrityFailed);
    }

    Ok(NvattestInstallation {
        binary,
        lib_dir,
        ca_bundle,
    })
}

/// Builds the local-verifier GPU-attestation invocation without a shell.
pub fn build_nvattest_attest_command(
    nvattest_dir: &Path,
    evidence_file: &Path,
    owner_nonce: &[u8],
    rim_store: &str,
    rim_dir: Option<&Path>,
) -> Result<NvattestCommand, GpuAppraisalReason> {
    if owner_nonce.len() != SPDM_NONCE_SIZE {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    if !matches!(rim_store, "remote" | "dir")
        || (rim_store == "dir" && rim_dir.is_none())
        || (rim_store == "remote" && rim_dir.is_some())
    {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }

    let installation = locate_nvattest(nvattest_dir)?;
    let mut argv = vec![installation.binary.clone().into_os_string()];
    argv.extend(
        [
            "--format",
            "json",
            "attest",
            "--device",
            "gpu",
            "--gpu-evidence-source",
            "file",
            "--gpu-evidence-file",
        ]
        .into_iter()
        .map(OsString::from),
    );
    argv.push(evidence_file.as_os_str().to_owned());
    argv.extend(
        ["--verifier", "local", "--rim-store"]
            .into_iter()
            .map(OsString::from),
    );
    argv.push(OsString::from(rim_store));
    argv.extend(["--ca-bundle"].into_iter().map(OsString::from));
    argv.push(installation.ca_bundle.clone().into_os_string());
    if let Some(rim_dir) = rim_dir {
        argv.extend(["--rim-dir"].into_iter().map(OsString::from));
        argv.push(rim_dir.as_os_str().to_owned());
    }
    argv.extend(["--nonce"].into_iter().map(OsString::from));
    argv.push(OsString::from(hex_lower(owner_nonce)));

    Ok(NvattestCommand {
        executable: installation.binary,
        argv,
        env: BTreeMap::from([(
            OsString::from("LD_LIBRARY_PATH"),
            installation.lib_dir.into_os_string(),
        )]),
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{build_nvattest_attest_command, locate_nvattest};
    use crate::error::GpuAppraisalReason;

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-spp-attest-binary-test-{}-{}",
                std::process::id(),
                NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn layout(root: &Path) {
        fs::create_dir_all(root.join("bin")).expect("create bin");
        fs::create_dir_all(root.join("lib")).expect("create lib");
        fs::create_dir_all(root.join("share/ca")).expect("create CA directory");
        fs::write(root.join("bin/nvattest"), "placeholder").expect("write binary");
        fs::write(root.join("share/ca/ca-bundle.pem"), "CA").expect("write CA bundle");
    }

    #[test]
    fn locate_rejects_missing_binary_and_library() {
        let root = TempDir::new();
        fs::create_dir_all(root.path()).expect("create root");
        assert_eq!(
            locate_nvattest(root.path()),
            Err(GpuAppraisalReason::NvattestUnavailable)
        );

        fs::create_dir_all(root.path().join("bin")).expect("create bin");
        fs::write(root.path().join("bin/nvattest"), "placeholder").expect("write binary");
        assert_eq!(
            locate_nvattest(root.path()),
            Err(GpuAppraisalReason::NvattestUnavailable)
        );
    }

    #[test]
    fn locate_rejects_missing_ca_bundle() {
        let root = TempDir::new();
        fs::create_dir_all(root.path()).expect("create root");
        fs::create_dir_all(root.path().join("bin")).expect("create bin");
        fs::create_dir_all(root.path().join("lib")).expect("create lib");
        fs::write(root.path().join("bin/nvattest"), "placeholder").expect("write binary");

        assert_eq!(
            locate_nvattest(root.path()),
            Err(GpuAppraisalReason::NvattestIntegrityFailed)
        );
    }

    #[test]
    fn build_command_uses_the_python_argv_shape() {
        let root = TempDir::new();
        layout(root.path());
        let evidence = root.path().join("evidence.json");
        let command =
            build_nvattest_attest_command(root.path(), &evidence, &[0xab; 32], "remote", None)
                .expect("build command");

        let argv = command
            .argv
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            argv,
            vec![
                root.path().join("bin/nvattest").display().to_string(),
                "--format".to_owned(),
                "json".to_owned(),
                "attest".to_owned(),
                "--device".to_owned(),
                "gpu".to_owned(),
                "--gpu-evidence-source".to_owned(),
                "file".to_owned(),
                "--gpu-evidence-file".to_owned(),
                evidence.display().to_string(),
                "--verifier".to_owned(),
                "local".to_owned(),
                "--rim-store".to_owned(),
                "remote".to_owned(),
                "--ca-bundle".to_owned(),
                root.path()
                    .join("share/ca/ca-bundle.pem")
                    .display()
                    .to_string(),
                "--nonce".to_owned(),
                "ab".repeat(32),
            ]
        );
        assert_eq!(
            command.env.get(OsStr::new("LD_LIBRARY_PATH")),
            Some(&root.path().join("lib").into_os_string())
        );
    }
}
