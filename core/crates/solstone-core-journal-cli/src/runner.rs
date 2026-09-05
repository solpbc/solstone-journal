// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

#[derive(Debug)]
pub enum NativeExecutableError {
    CurrentExe(std::io::Error),
    Missing { path: PathBuf },
    NonExecutable { path: PathBuf },
}

impl std::fmt::Display for NativeExecutableError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentExe(error) => write!(
                formatter,
                "native-helper-current-exe: could not inspect the native journal executable: {error}"
            ),
            Self::Missing { path } => write!(
                formatter,
                "native-helper-missing: {}. Reinstall solstone-journal.",
                path.display()
            ),
            Self::NonExecutable { path } => write!(
                formatter,
                "native-helper-not-executable: {}. Reinstall solstone-journal.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for NativeExecutableError {}

pub(crate) fn sibling_native_for_current_executable(
    binary: &str,
) -> Result<PathBuf, NativeExecutableError> {
    let executable = std::env::current_exe().map_err(NativeExecutableError::CurrentExe)?;
    sibling_native_for_executable(&executable, binary)
}

pub(crate) fn sibling_native_for_executable(
    executable: &Path,
    binary: &str,
) -> Result<PathBuf, NativeExecutableError> {
    sibling_native_in_dir(&executable_dir(executable), binary)
}

pub fn sibling_native_in_dir(dir: &Path, binary: &str) -> Result<PathBuf, NativeExecutableError> {
    let candidate = dir.join(binary);
    match fs::metadata(&candidate) {
        Ok(_) if is_executable(&candidate) => Ok(candidate),
        Ok(_) => Err(NativeExecutableError::NonExecutable { path: candidate }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(NativeExecutableError::Missing { path: candidate })
        }
        Err(_) => Err(NativeExecutableError::NonExecutable { path: candidate }),
    }
}

pub(crate) fn native_process_args(
    spec: &crate::processes::NativeProcessSpec,
    owner_argv: &[OsString],
) -> Vec<OsString> {
    spec.preset_argv
        .iter()
        .map(OsString::from)
        .chain(owner_argv.iter().cloned())
        .collect()
}

pub(crate) fn exec_process(program: &OsStr, args: &[OsString]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(program);
        command.args(args);
        Err(command.exec())
    }
    #[cfg(not(unix))]
    {
        let _ = (program, args);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "native journal Python process replacement is unavailable on this platform",
        ))
    }
}

fn executable_dir(executable: &Path) -> PathBuf {
    executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = reserve_temp_path("solstone-core-journal-cli-runner");
            fs::create_dir(&path).expect("create temporary test directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, "#!/bin/sh\nexit 0\n").expect("write interpreter fixture");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make interpreter fixture executable");
    }

    #[cfg(unix)]
    #[test]
    fn sibling_native_requires_the_named_executable() {
        let temp = TempDir::new();
        let executable = temp.path.join("solstone-core-journal");
        fs::write(&executable, "native executable").expect("write executable fixture");
        for binary in ["solstone-core-depict", "solstone-core-describe"] {
            let helper = temp.path.join(binary);
            make_executable(&helper);
            assert_eq!(
                sibling_native_for_executable(&executable, binary)
                    .expect("named native helper should be selected"),
                helper
            );
        }
        assert!(matches!(
            sibling_native_for_executable(&executable, "missing-helper"),
            Err(NativeExecutableError::Missing { path }) if path == temp.path.join("missing-helper")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn native_process_args_preserve_non_utf8_owner_arguments() {
        use std::os::unix::ffi::OsStringExt;

        let spec = crate::processes::native_process_spec_for("schedule")
            .expect("schedule NativeProcessSpec");
        let owner = vec![OsString::from_vec(vec![0xff])];
        let args = native_process_args(spec, &owner);
        assert_eq!(args.first(), Some(&OsString::from("schedule")));
        assert_eq!(args.last(), owner.last());
    }
}
