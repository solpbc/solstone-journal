// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact, read-only recognition of launchers shipped by the retired V1 line.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const LAUNCHER_LIMIT: u64 = 32 * 1024;

pub(crate) fn validate_effective_path(
    home: &Path,
    current_dir: &Path,
    executable_dir: &Path,
) -> Result<(), String> {
    let Some(path) = std::env::var_os("PATH") else {
        return Ok(());
    };
    validate_path_value(home, current_dir, executable_dir, &path)
}

fn validate_path_value(
    home: &Path,
    current_dir: &Path,
    executable_dir: &Path,
    path: &std::ffi::OsStr,
) -> Result<(), String> {
    for command in ["solstone", "journal"] {
        for component in std::env::split_paths(path) {
            let directory = if component.as_os_str().is_empty() {
                current_dir.to_path_buf()
            } else if component.is_absolute() {
                component
            } else {
                return Err(format!(
                    "PATH contains a relative directory before the V2 {command} command"
                ));
            };
            let candidate = directory.join(command);
            let metadata = match fs::metadata(&candidate) {
                Ok(metadata) => metadata,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    continue;
                }
                Err(error) => {
                    return Err(format!(
                        "inspect PATH candidate {}: {error}",
                        candidate.display()
                    ));
                }
            };
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
            let resolved = fs::canonicalize(&candidate).map_err(|error| {
                format!("resolve PATH candidate {}: {error}", candidate.display())
            })?;
            let allowed_runtime = fs::canonicalize(executable_dir.join(command)).ok();
            let allowed_public = fs::canonicalize(home.join(".local/bin").join(command)).ok();
            if allowed_runtime.as_ref() == Some(&resolved)
                || allowed_public.as_ref() == Some(&resolved)
            {
                break;
            }
            return Err(format!(
                "PATH resolves {command} to {} before the V2 installation",
                candidate.display()
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LegacyLauncherFamily {
    Python,
    NativeRoot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LegacyLauncher {
    pub family: LegacyLauncherFamily,
    pub installation_bin: PathBuf,
    pub public_link: Option<PathBuf>,
    pub resolved_path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub bytes: Vec<u8>,
}

impl LegacyLauncher {
    pub(crate) fn same_installation(&self, other: &Self) -> bool {
        self.family == other.family && self.installation_bin == other.installation_bin
    }
}

pub(crate) fn classify(
    home: &Path,
    public_path: &Path,
    command: &str,
) -> Result<Option<LegacyLauncher>, String> {
    let public_metadata = match fs::symlink_metadata(public_path) {
        Ok(metadata) => metadata,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(format!("inspect {}: {error}", public_path.display())),
    };
    let public_link = if public_metadata.file_type().is_symlink() {
        Some(
            fs::read_link(public_path)
                .map_err(|error| format!("read {}: {error}", public_path.display()))?,
        )
    } else if public_metadata.file_type().is_file() {
        None
    } else {
        return Ok(None);
    };
    let resolved_path = fs::canonicalize(public_path)
        .map_err(|error| format!("resolve {}: {error}", public_path.display()))?;
    let resolved_home = fs::canonicalize(home)
        .map_err(|error| format!("resolve owner home {}: {error}", home.display()))?;
    if !resolved_path.starts_with(&resolved_home) {
        return Ok(None);
    }
    let metadata = fs::metadata(&resolved_path)
        .map_err(|error| format!("inspect {}: {error}", resolved_path.display()))?;
    if !metadata.is_file()
        || metadata.len() > LAUNCHER_LIMIT
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Ok(None);
    }
    let bytes = fs::read(&resolved_path)
        .map_err(|error| format!("read {}: {error}", resolved_path.display()))?;
    if bytes.len() as u64 > LAUNCHER_LIMIT {
        return Ok(None);
    }
    let family = if python_launcher(command, &bytes) {
        LegacyLauncherFamily::Python
    } else if native_root_launcher(command, &bytes) {
        LegacyLauncherFamily::NativeRoot
    } else {
        return Ok(None);
    };
    let Some(installation_bin) = resolved_path.parent().map(Path::to_path_buf) else {
        return Ok(None);
    };
    Ok(Some(LegacyLauncher {
        family,
        installation_bin,
        public_link,
        resolved_path,
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.permissions().mode(),
        bytes,
    }))
}

fn python_launcher(command: &str, bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Some((shebang, body)) = text.split_once('\n') else {
        return false;
    };
    let Some(interpreter) = shebang.strip_prefix("#!") else {
        return false;
    };
    let interpreter = Path::new(interpreter);
    if !interpreter.is_absolute()
        || !interpreter
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("python"))
    {
        return false;
    }
    let Some((module, function)) = python_entrypoint(command) else {
        return false;
    };
    body == generated_console_script(module, function, '"')
        || body == generated_console_script(module, function, '\'')
        || body == generated_console_script_re(module, function)
}

fn python_entrypoint(command: &str) -> Option<(&'static str, &'static str)> {
    match command {
        "sol" | "solstone" => Some(("solstone.think.sol_cli", "main")),
        "journal" => Some(("solstone.think.sol_cli", "journal_main")),
        "mlx-vlm-server" => Some(("solstone.think.providers.mlx_server", "main")),
        _ => None,
    }
}

fn generated_console_script(module: &str, function: &str, quote: char) -> String {
    format!(
        "# -*- coding: utf-8 -*-\nimport sys\nfrom {module} import {function}\nif __name__ == {quote}__main__{quote}:\n    if sys.argv[0].endswith({quote}-script.pyw{quote}):\n        sys.argv[0] = sys.argv[0][:-11]\n    elif sys.argv[0].endswith({quote}.exe{quote}):\n        sys.argv[0] = sys.argv[0][:-4]\n    sys.exit({function}())\n"
    )
}

fn generated_console_script_re(module: &str, function: &str) -> String {
    format!(
        "# -*- coding: utf-8 -*-\nimport re\nimport sys\nfrom {module} import {function}\nif __name__ == '__main__':\n    sys.argv[0] = re.sub(r'(-script\\.pyw|\\.exe)?$', '', sys.argv[0])\n    sys.exit({function}())\n"
    )
}

fn native_root_launcher(command: &str, bytes: &[u8]) -> bool {
    matches!(command, "sol" | "solstone") && bytes == native_root_launcher_bytes(command).as_bytes()
}

fn native_root_launcher_bytes(command: &str) -> String {
    format!(
        r#"#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
id={command}

p=$0
n=0
while [ -L "$p" ]; do
    n=$((n + 1))
    if [ "$n" -gt 40 ]; then
        printf '%s: native launcher symlink cycle detected. Reinstall solstone and solstone-core.\n' "$id" >&2
        exit 78
    fi
    t=$(readlink -- "$p") || {{
        printf '%s: native launcher symlink is dangling: %s. Reinstall solstone and solstone-core.\n' "$id" "$p" >&2
        exit 78
    }}
    case $t in
        /*) p=$t ;;
        *) p=$(dirname -- "$p")/$t ;;
    esac
done

if [ ! -f "$p" ]; then
    printf '%s: native launcher symlink is dangling: %s. Reinstall solstone and solstone-core.\n' "$id" "$p" >&2
    exit 78
fi

d=$(cd -P -- "$(dirname -- "$p")" && pwd) || {{
    printf '%s: native launcher symlink is dangling: %s. Reinstall solstone and solstone-core.\n' "$id" "$p" >&2
    exit 78
}}
core=$d/solstone-core

if [ ! -e "$core" ]; then
    printf '%s: native solstone-core sibling is missing: %s. Reinstall solstone and solstone-core.\n' "$id" "$core" >&2
    exit 78
fi
if [ ! -f "$core" ] || [ ! -x "$core" ]; then
    printf '%s: native solstone-core sibling is not executable: %s. Reinstall solstone and solstone-core.\n' "$id" "$core" >&2
    exit 78
fi

exec "$core" "__solstone_identity=$id" "$@"
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn root(name: &str) -> PathBuf {
        let path = PathBuf::from("/var/tmp").join(format!(
            "solstone-legacy-launcher-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_executable(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn recognizes_the_shipped_python_console_scripts_and_rejects_near_twins() {
        let root = root("python");
        let home = root.join("home");
        let bin = home.join(".local/share/uv/tools/solstone/bin");
        let target = bin.join("solstone");
        let body = format!(
            "#!{}\n{}",
            bin.join("python3").display(),
            generated_console_script("solstone.think.sol_cli", "main", '"')
        );
        write_executable(&target, &body);
        let public = home.join(".local/bin/solstone");
        fs::create_dir_all(public.parent().unwrap()).unwrap();
        symlink(&target, &public).unwrap();
        let found = classify(&home, &public, "solstone").unwrap().unwrap();
        assert_eq!(found.family, LegacyLauncherFamily::Python);
        assert_eq!(found.installation_bin, bin);

        write_executable(&target, &body.replace(" import main", " import main2"));
        assert!(classify(&home, &public, "solstone").unwrap().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recognizes_the_exact_shipped_native_root_launchers_only() {
        let root = root("native");
        let home = root.join("home");
        let target = home.join(".local/share/uv/tools/solstone/bin/solstone");
        write_executable(&target, &native_root_launcher_bytes("solstone"));
        let public = home.join(".local/bin/solstone");
        fs::create_dir_all(public.parent().unwrap()).unwrap();
        symlink(&target, &public).unwrap();
        assert_eq!(
            classify(&home, &public, "solstone")
                .unwrap()
                .unwrap()
                .family,
            LegacyLauncherFamily::NativeRoot
        );
        let mut near = native_root_launcher_bytes("solstone");
        near.push('\n');
        write_executable(&target, &near);
        assert!(classify(&home, &public, "solstone").unwrap().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pipx_layout_classifies_identically_to_the_uv_tool_layout() {
        // classify() identifies a v1 artifact purely by (a) the resolved
        // real path staying under the owner's home, and (b) an exact byte
        // match against a known-shipped launcher shape. It never reads
        // uv-receipt.toml, pipx_metadata.json, or a *.dist-info/RECORD file,
        // so it is installer-agnostic by construction. This pins that: the
        // identical launcher bytes under a pipx-shaped directory classify
        // exactly the same as the uv-tool-shaped directory above.
        let root = root("pipx");
        let home = root.join("home");
        let target = home.join(".local/pipx/venvs/solstone/bin/solstone");
        write_executable(&target, &native_root_launcher_bytes("solstone"));
        let public = home.join(".local/bin/solstone");
        fs::create_dir_all(public.parent().unwrap()).unwrap();
        symlink(&target, &public).unwrap();
        let found = classify(&home, &public, "solstone").unwrap().unwrap();
        assert_eq!(found.family, LegacyLauncherFamily::NativeRoot);
        assert_eq!(found.installation_bin, home.join(".local/pipx/venvs/solstone/bin"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn outside_home_and_app_owned_launchers_are_not_legacy() {
        let root = root("outside");
        let home = root.join("home");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let outside = root.join("outside/solstone");
        write_executable(&outside, &native_root_launcher_bytes("solstone"));
        let public = home.join(".local/bin/solstone");
        symlink(&outside, &public).unwrap();
        assert!(classify(&home, &public, "solstone").unwrap().is_none());

        fs::remove_file(&public).unwrap();
        write_executable(
            &public,
            "#!/bin/sh\n# managed-version: app-owned-child\nexec '/missing' \"$@\"\n",
        );
        assert!(classify(&home, &public, "solstone").unwrap().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_allows_no_hit_runtime_or_canonical_and_refuses_an_earlier_foreign_hit() {
        let root = root("path");
        let home = root.join("home");
        let runtime = root.join("runtime");
        let foreign = root.join("foreign");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&foreign).unwrap();

        validate_path_value(
            &home,
            &root,
            &runtime,
            std::ffi::OsStr::new("/usr/bin:/bin"),
        )
        .unwrap();

        for command in ["solstone", "journal"] {
            write_executable(&runtime.join(command), "#!/bin/sh\nexit 0\n");
        }
        validate_path_value(
            &home,
            &root,
            &runtime,
            std::ffi::OsStr::new(&format!("{}:/usr/bin", runtime.display())),
        )
        .unwrap();

        write_executable(&foreign.join("journal"), "#!/bin/sh\nexit 0\n");
        assert!(
            validate_path_value(
                &home,
                &root,
                &runtime,
                std::ffi::OsStr::new(&format!(
                    "{}:{}:/usr/bin",
                    foreign.display(),
                    runtime.display()
                )),
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(root);
    }
}
