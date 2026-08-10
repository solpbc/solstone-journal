// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::processes::ProcessSpec;

pub(crate) const PYTHON_BOOTSTRAP_SCRIPT: &str = "import importlib, logging, sys\nmodule = sys.argv[1]\ndisplay_argv0 = sys.argv[2]\nverbose_marker = sys.argv[3]\nif verbose_marker == \"1\":\n    logging.basicConfig(level=logging.DEBUG)\nsys.argv = [display_argv0, *sys.argv[4:]]\nresult = importlib.import_module(module).main()\nsys.exit(0 if result is None else int(result))\n";

#[derive(Debug)]
pub(crate) enum InterpreterError {
    CurrentExe(std::io::Error),
    Missing { dir: PathBuf },
    NonExecutable { path: PathBuf },
}

impl std::fmt::Display for InterpreterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentExe(error) => write!(
                formatter,
                "could not inspect the native journal executable: {error}"
            ),
            Self::Missing { dir } => write!(
                formatter,
                "native journal Python is missing beside {}. Reinstall solstone and solstone-core.",
                dir.display()
            ),
            Self::NonExecutable { path } => write!(
                formatter,
                "native journal Python is not executable: {}. Reinstall solstone and solstone-core.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for InterpreterError {}

#[derive(Debug)]
pub(crate) enum NativeExecutableError {
    CurrentExe(std::io::Error),
    Missing { path: PathBuf },
    NonExecutable { path: PathBuf },
}

impl std::fmt::Display for NativeExecutableError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentExe(error) => write!(
                formatter,
                "could not inspect the native journal executable: {error}"
            ),
            Self::Missing { path } => write!(
                formatter,
                "native journal helper is missing: {}. Reinstall solstone-journal.",
                path.display()
            ),
            Self::NonExecutable { path } => write!(
                formatter,
                "native journal helper is not executable: {}. Reinstall solstone-journal.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for NativeExecutableError {}

pub(crate) fn sibling_python_for_current_executable() -> Result<PathBuf, InterpreterError> {
    let executable = std::env::current_exe().map_err(InterpreterError::CurrentExe)?;
    sibling_python_for_executable(&executable)
}

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
    let candidate = executable_dir(executable).join(binary);
    match fs::metadata(&candidate) {
        Ok(_) if is_executable(&candidate) => Ok(candidate),
        Ok(_) => Err(NativeExecutableError::NonExecutable { path: candidate }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(NativeExecutableError::Missing { path: candidate })
        }
        Err(_) => Err(NativeExecutableError::NonExecutable { path: candidate }),
    }
}

pub(crate) fn sibling_python_for_executable(
    executable: &Path,
) -> Result<PathBuf, InterpreterError> {
    let dir = executable_dir(executable);
    for name in ["python3", "python"] {
        let candidate = dir.join(name);
        match fs::metadata(&candidate) {
            Ok(_) if is_executable(&candidate) => return Ok(candidate),
            Ok(_) => return Err(InterpreterError::NonExecutable { path: candidate }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InterpreterError::NonExecutable { path: candidate }),
        }
    }
    Err(InterpreterError::Missing { dir })
}

pub(crate) fn process_args(
    spec: &ProcessSpec,
    verbose: bool,
    owner_argv: &[OsString],
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-c"),
        OsString::from(PYTHON_BOOTSTRAP_SCRIPT),
        OsString::from(spec.module),
        OsString::from(format!("journal {}", spec.token)),
        OsString::from(if verbose { "1" } else { "0" }),
    ];
    args.extend(spec.preset_argv.iter().map(OsString::from));
    args.extend(owner_argv.iter().cloned());
    args
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
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be available")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-core-journal-cli-runner-{}-{stamp}",
                std::process::id()
            ));
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
    fn sibling_interpreter_prefers_python3_and_rejects_non_executables() {
        let temp = TempDir::new();
        let executable = temp.path.join("solstone-core");
        fs::write(&executable, "native executable").expect("write executable fixture");
        make_executable(&temp.path.join("python"));
        make_executable(&temp.path.join("python3"));
        assert_eq!(
            sibling_python_for_executable(&executable).expect("python3 should be selected"),
            temp.path.join("python3")
        );

        fs::remove_file(temp.path.join("python3")).expect("remove python3 fixture");
        fs::write(temp.path.join("python3"), "not executable").expect("write non-executable");
        assert!(matches!(
            sibling_python_for_executable(&executable),
            Err(InterpreterError::NonExecutable { path }) if path == temp.path.join("python3")
        ));

        fs::remove_file(temp.path.join("python3")).expect("remove non-executable");
        fs::remove_file(temp.path.join("python")).expect("remove python");
        assert!(matches!(
            sibling_python_for_executable(&executable),
            Err(InterpreterError::Missing { dir }) if dir == temp.path
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sibling_native_requires_the_named_executable() {
        let temp = TempDir::new();
        let executable = temp.path.join("solstone-core-journal");
        fs::write(&executable, "native executable").expect("write executable fixture");
        let helper = temp.path.join("solstone-core-depict");
        make_executable(&helper);
        assert_eq!(
            sibling_native_for_executable(&executable, "solstone-core-depict")
                .expect("named native helper should be selected"),
            helper
        );
        assert!(matches!(
            sibling_native_for_executable(&executable, "missing-helper"),
            Err(NativeExecutableError::Missing { path }) if path == temp.path.join("missing-helper")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn process_args_preserves_non_utf8_owner_arguments() {
        use std::os::unix::ffi::OsStringExt;

        let spec = crate::processes::process_spec_for("up").expect("up ProcessSpec");
        let owner = vec![OsString::from_vec(vec![0xff])];
        let args = process_args(spec, false, &owner);
        assert_eq!(args.last(), owner.last());
    }
}
