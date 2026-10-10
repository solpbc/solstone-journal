// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-local Windows verifier admission and child process policy.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::error::GpuAppraisalReason;

#[cfg(windows)]
pub(super) const BINARY: &str = "lib/solstone-nvattest/nvattest.exe";
#[cfg(windows)]
const CA_BUNDLE: &str = "share/ca/ca-bundle.pem";
#[cfg(windows)]
const RUNTIME: [&str; 3] = [
    "lib/solstone-nvattest/msvcp140.dll",
    "lib/solstone-nvattest/vcruntime140.dll",
    "lib/solstone-nvattest/vcruntime140_1.dll",
];

pub(super) fn child_path(path: &Path) -> Result<PathBuf, GpuAppraisalReason> {
    let text = path
        .to_str()
        .ok_or(GpuAppraisalReason::NvattestUnavailable)?;
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    // The verifier's native libraries use ordinary Win32 file APIs. Refuse
    // network paths and names beyond MAX_PATH before starting the helper.
    let bytes = text.as_bytes();
    let drive_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    if !drive_absolute
        || text.starts_with(r"UNC\")
        || text.starts_with(r"\\")
        || text.encode_utf16().count() > 259
    {
        return Err(GpuAppraisalReason::NvattestUnavailable);
    }
    Ok(PathBuf::from(text))
}

pub(super) fn environment(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<BTreeMap<OsString, OsString>, GpuAppraisalReason> {
    let mut result = BTreeMap::new();
    for (name, value) in parent {
        let Some(name) = name.to_str() else {
            continue;
        };
        // Windows environment names are case insensitive. Keep only the OS
        // root (loader/search paths) and temporary directories (native scratch).
        let canonical = match name.to_ascii_lowercase().as_str() {
            "systemroot" => "SystemRoot",
            "temp" => "TEMP",
            "tmp" => "TMP",
            _ => continue,
        };
        result.insert(OsString::from(canonical), value);
    }
    let root = result
        .get(&OsString::from("SystemRoot"))
        .and_then(|value| value.to_str())
        .filter(|root| !root.is_empty() && !root.contains(';'))
        .ok_or(GpuAppraisalReason::NvattestUnavailable)?;
    child_path(Path::new(root))?;
    result.insert(
        OsString::from("PATH"),
        OsString::from(format!(r"{root}\System32;{root}")),
    );
    // No OpenSSL configuration/provider, certificate, NVAT, or proxy variables
    // reach the child. An online check needing an ambient proxy fails closed.
    Ok(result)
}

/// The package root of a verifier executable admitted at `relative`.
///
/// The verifier sits in a private folder, so the root is as many levels up
/// as `relative` has components, not a fixed two. An executable that is not
/// at `relative` has no package root.
pub(super) fn package_root<'a>(
    executable: &'a Path,
    relative: &str,
) -> Result<&'a Path, GpuAppraisalReason> {
    let relative = Path::new(relative);
    if !executable.ends_with(relative) {
        return Err(GpuAppraisalReason::NvattestUnavailable);
    }
    executable
        .ancestors()
        .nth(relative.components().count())
        .ok_or(GpuAppraisalReason::NvattestUnavailable)
}

#[cfg(windows)]
pub(super) fn locate(root: &Path) -> Result<super::NvattestInstallation, GpuAppraisalReason> {
    let root = std::fs::canonicalize(root).map_err(|_| GpuAppraisalReason::NvattestUnavailable)?;
    let root = child_path(&root)?;
    if !root.join(BINARY).is_file() {
        return Err(GpuAppraisalReason::NvattestUnavailable);
    }
    let payload = solstone_core_distribution::windows_payload::verify_windows_payload(&root)
        .map_err(|_| GpuAppraisalReason::NvattestIntegrityFailed)?;
    let member = |path: &str| {
        let path = payload
            .declared_path(path)
            .map_err(|_| GpuAppraisalReason::NvattestIntegrityFailed)?;
        child_path(&path)
    };
    // Inventory admission checks every non-system DLL before the first spawn;
    // these three must also be present so the loader cannot fall back to a
    // machine-wide VC runtime. The CA is pinned even for offline appraisals.
    for runtime in RUNTIME {
        member(runtime)?;
    }
    let binary = member(BINARY)?;
    let ca_bundle = member(CA_BUNDLE)?;
    let lib_dir = child_path(&root.join("lib/solstone-nvattest"))?;
    Ok(super::NvattestInstallation {
        binary,
        lib_dir,
        ca_bundle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn child_environment_discards_ambient_configuration_case_insensitively() {
        let parent = [
            ("systemroot", r"C:\Windows"),
            ("Path", r"C:\hostile"),
            ("OPENSSL_CONF", "evil.cnf"),
            ("openssl_modules", "evil"),
            ("SSL_CERT_DIR", "evil"),
            ("CURL_CA_BUNDLE", "evil"),
            ("NVAT_X", "evil"),
            ("HTTPS_PROXY", "evil"),
            ("temp", r"C:\Temp"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let actual = environment(parent).unwrap();
        assert_eq!(actual.len(), 3);
        assert_eq!(actual[OsStr::new("SystemRoot")], r"C:\Windows");
        assert_eq!(
            actual[OsStr::new("PATH")],
            r"C:\Windows\System32;C:\Windows"
        );
        assert_eq!(actual[OsStr::new("TEMP")], r"C:\Temp");
        assert!(environment([(OsString::from("Path"), OsString::from("evil"))]).is_err());
    }

    #[test]
    fn package_root_climbs_out_of_the_private_verifier_folder() {
        let relative = "lib/solstone-nvattest/nvattest.exe";
        let executable = Path::new("/pkg ü/lib/solstone-nvattest/nvattest.exe");
        assert_eq!(
            package_root(executable, relative).unwrap(),
            Path::new("/pkg ü")
        );
        // The pre-relocation location is not an admitted verifier path.
        assert!(package_root(Path::new("/pkg/bin/nvattest.exe"), relative).is_err());
        assert!(package_root(Path::new("nvattest.exe"), relative).is_err());
    }

    #[test]
    fn child_paths_refuse_network_paths_and_count_utf16_units() {
        assert_eq!(
            child_path(Path::new(r"\\?\C:\space ü\nvattest.exe")).unwrap(),
            Path::new(r"C:\space ü\nvattest.exe")
        );
        for path in [
            r"\\?\UNC\host\share\nvattest.exe",
            r"\\host\share\nvattest.exe",
        ] {
            assert!(child_path(Path::new(path)).is_err());
        }
        assert!(child_path(Path::new(&format!("C:\\{}", "x".repeat(256)))).is_ok());
        assert!(child_path(Path::new(&format!("C:\\{}", "x".repeat(257)))).is_err());
        assert!(child_path(Path::new(&format!("C:\\{}", "🦀".repeat(129)))).is_err());
    }
}
