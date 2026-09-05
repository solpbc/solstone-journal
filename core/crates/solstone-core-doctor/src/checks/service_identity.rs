// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    checks::managed_wrapper::{parse_sol_bin, resolve_non_strict},
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

const REPAIR: &str = "run journal setup to reinstall the service";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if context.platform == crate::vocabulary::Platform::Windows {
        return Ok(make_result(
            check,
            Status::Skip,
            "not supported on windows",
            None::<String>,
        ));
    }
    let Some(path) = service_path(context) else {
        return Ok(make_result(
            check,
            Status::Skip,
            "not supported on windows",
            None::<String>,
        ));
    };
    if !path.exists() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal service",
            None::<String>,
        ));
    }
    let parts = match context.platform {
        crate::vocabulary::Platform::Darwin => launchd_program_arguments(&path),
        crate::vocabulary::Platform::Linux => systemd_exec_start_parts(&path),
        crate::vocabulary::Platform::Windows => {
            return Ok(make_result(
                check,
                Status::Skip,
                "not supported on windows",
                None::<String>,
            ));
        }
    };
    let Some(parts) = parts.filter(|parts| !parts.is_empty()) else {
        return Ok(make_result(
            check,
            Status::Fail,
            format!(
                "service unit is malformed: no executable target in {}",
                path.display()
            ),
            Some(REPAIR),
        ));
    };
    let raw = &parts[0];
    let resolved = resolve_service_target(raw);
    let expected = resolve_non_strict(&context.install_bin_dir.join("journal"));
    if resolved == expected {
        return Ok(make_result(
            check,
            Status::Ok,
            format!(
                "service target matches current install: {raw} -> {}",
                resolved.display()
            ),
            None::<String>,
        ));
    }
    Ok(make_result(
        check,
        Status::Fail,
        format!(
            "service target mismatch: {raw} resolves to {}, expected {}",
            resolved.display(),
            expected.display()
        ),
        Some("run journal setup --force from this install to refresh the service"),
    ))
}

fn service_path(context: &CheckContext) -> Option<PathBuf> {
    match context.platform {
        crate::vocabulary::Platform::Darwin => Some(
            context
                .home_dir
                .join("Library/LaunchAgents/org.solpbc.solstone.plist"),
        ),
        crate::vocabulary::Platform::Linux => Some(
            context
                .home_dir
                .join(".config/systemd/user/solstone.service"),
        ),
        crate::vocabulary::Platform::Windows => None,
    }
}

fn launchd_program_arguments(path: &Path) -> Option<Vec<String>> {
    plist::Value::from_file(path)
        .ok()?
        .as_dictionary()?
        .get("ProgramArguments")?
        .as_array()?
        .iter()
        .map(|value| value.as_string().map(str::to_owned))
        .collect()
}

fn systemd_exec_start_parts(path: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(path).ok()?;
    let value = text
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))?;
    split_exec_start(value)
}

fn split_exec_start(value: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match quote {
            Some('"') if character == '\\' => escaped = true,
            Some(delimiter) if character == delimiter => quote = None,
            Some(_) => current.push(character),
            None if matches!(character, '\'' | '"') => quote = Some(character),
            None if character == '\\' => escaped = true,
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            None => current.push(character),
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Some(parts)
}

fn resolve_service_target(raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_symlink()
        && let Ok(target) = fs::read_link(path)
    {
        let target = if target.is_absolute() {
            target
        } else {
            path.parent().unwrap_or_else(|| Path::new(".")).join(target)
        };
        return resolve_non_strict(&target);
    }
    if path.is_file()
        && let Ok(content) = fs::read_to_string(path)
        && let Some(sol_bin) = parse_sol_bin(&content)
    {
        return resolve_non_strict(&sol_bin);
    }
    resolve_non_strict(path)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::*;
    use crate::{
        checks::test_support::{check, context},
        vocabulary::{Platform, Severity, Status},
    };

    fn executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").expect("write executable");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
    }

    fn v7_journal_wrapper(journal: &Path, target: &Path) -> String {
        format!(
            "#!/bin/bash\n# journal — managed by 'journal config'. Edits will be overwritten.\n# managed-version: 7\n: \"${{SOLSTONE_JOURNAL:={}}}\"\nexport SOLSTONE_JOURNAL\nSOL_BIN='{}'\n# Warn when pyproject.toml or uv.lock is newer than .installed.\n# Skipped silently if .installed is absent.\nREPO_ROOT=\"${{SOL_BIN%/.venv/bin/journal}}\"\nif [ -f \"$REPO_ROOT/.installed\" ]; then\n  if [ \"$REPO_ROOT/pyproject.toml\" -nt \"$REPO_ROOT/.installed\" ] \\\n     || [ \"$REPO_ROOT/uv.lock\" -nt \"$REPO_ROOT/.installed\" ]; then\n    echo \"journal: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install\" >&2\n  fi\nfi\nif [ ! -x \"$SOL_BIN\" ]; then\n    printf 'journal: venv binary missing or not executable: %s\\n' \"$SOL_BIN\" >&2\n    exit 127\nfi\nexec \"$SOL_BIN\" \"$@\"\n",
            journal.display(),
            target.display().to_string().replace('\'', "'\\''")
        )
    }

    #[test]
    fn resolves_linux_managed_wrappers_and_reports_malformed_units() {
        let staged = context();
        let journal = staged.install_bin_dir.join("journal");
        fs::create_dir_all(&staged.install_bin_dir).expect("create install bin");
        executable(&journal);
        let wrapper = staged.home_dir.join(".local/bin/journal");
        fs::create_dir_all(wrapper.parent().expect("wrapper parent"))
            .expect("create wrapper parent");
        fs::write(&wrapper, v7_journal_wrapper(&staged.journal_path, &journal))
            .expect("write managed wrapper");
        let unit = staged
            .home_dir
            .join(".config/systemd/user/solstone.service");
        fs::create_dir_all(unit.parent().expect("unit parent")).expect("create unit parent");
        fs::write(
            &unit,
            format!("ExecStart={} start 5015\n", wrapper.display()),
        )
        .expect("write service unit");
        let check = check("service_identity", Severity::Blocker);
        assert_eq!(run(&staged, check).unwrap().status, Status::Ok);

        fs::write(&unit, "ExecStart='unterminated\n").expect("write malformed unit");
        assert_eq!(run(&staged, check).unwrap().status, Status::Fail);
    }

    #[test]
    fn reads_full_launchd_program_arguments() {
        let mut staged = context();
        staged.context.platform = Platform::Darwin;
        let journal = staged.install_bin_dir.join("journal");
        fs::create_dir_all(&staged.install_bin_dir).expect("create install bin");
        executable(&journal);
        let path = staged
            .home_dir
            .join("Library/LaunchAgents/org.solpbc.solstone.plist");
        fs::create_dir_all(path.parent().expect("plist parent")).expect("create plist parent");
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![
                plist::Value::String(journal.display().to_string()),
                plist::Value::String("start".into()),
                plist::Value::String("5015".into()),
            ]),
        );
        plist::Value::Dictionary(dictionary)
            .to_file_xml(&path)
            .expect("write plist");
        assert_eq!(
            run(&staged, check("service_identity", Severity::Blocker))
                .unwrap()
                .status,
            Status::Ok
        );
    }
}
