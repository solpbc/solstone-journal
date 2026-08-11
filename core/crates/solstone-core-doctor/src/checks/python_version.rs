// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use solstone_core_journal::installed_site_packages_from_executable_dir;

use crate::{
    checks::package_metadata::{SOLSTONE, installed},
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

const DEFAULT_REQUIRES_PYTHON: &str = ">=3.11";
const PYTHON_VERSION_FIX: &str = "install Python >=3.11, then retry";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let Some(site_packages) = installed_site_packages_from_executable_dir(&context.install_bin_dir)
    else {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "could not resolve site-packages from install bin directory: {}",
                context.install_bin_dir.display()
            ),
            None::<String>,
        ));
    };
    let spec = installed(context, &[SOLSTONE])?
        .and_then(|distributions| {
            distributions
                .get(SOLSTONE)
                .and_then(|item| item.requires_python.clone())
        })
        .filter(|spec| minimum_version(spec).is_some())
        .unwrap_or_else(|| DEFAULT_REQUIRES_PYTHON.into());
    let Some(minimum) = minimum_version(&spec) else {
        return Ok(make_result(
            check,
            Status::Fail,
            format!("unsupported requires-python specifier: {spec}"),
            Some(PYTHON_VERSION_FIX),
        ));
    };
    let Some((current, exact)) = current_python_version(&context.install_bin_dir, &site_packages)
    else {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "could not resolve Python version from install prefix: {}",
                context
                    .install_bin_dir
                    .parent()
                    .unwrap_or(&context.install_bin_dir)
                    .display()
            ),
            None::<String>,
        ));
    };
    let comparison = if exact {
        current.cmp(&minimum)
    } else {
        current[..2].cmp(&minimum[..2])
    };
    let precision = if exact {
        ""
    } else {
        " (patch version unavailable without pyvenv.cfg; comparing major.minor only)"
    };
    let current = version_text(current);
    if comparison.is_lt() {
        return Ok(make_result(
            check,
            Status::Fail,
            format!("python {current} does not satisfy {spec}{precision}"),
            Some(PYTHON_VERSION_FIX),
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        format!("python {current} satisfies {spec}{precision}"),
        None::<String>,
    ))
}

fn current_python_version(
    install_bin_dir: &Path,
    site_packages: &Path,
) -> Option<([u32; 3], bool)> {
    let prefix = install_bin_dir.parent()?;
    if let Ok(config) = fs::read_to_string(prefix.join("pyvenv.cfg"))
        && let Some(version) = config.lines().find_map(parse_venv_version)
    {
        return Some((version, true));
    }
    let python_dir = site_packages.parent()?.file_name()?.to_str()?;
    parse_python_directory(python_dir).map(|[major, minor]| ([major, minor, 0], false))
}

fn parse_venv_version(line: &str) -> Option<[u32; 3]> {
    let (name, value) = line.split_once('=')?;
    (name.trim() == "version").then_some(())?;
    parse_version_prefix(value.trim())
}

fn parse_python_directory(name: &str) -> Option<[u32; 2]> {
    let value = name.strip_prefix("python")?;
    let (major, minor) = value.split_once('.')?;
    Some([major.parse().ok()?, minor.parse().ok()?])
}

fn parse_version_prefix(value: &str) -> Option<[u32; 3]> {
    let mut parts = value.split('.');
    let major = numeric_component(parts.next()?)?;
    let minor = numeric_component(parts.next()?)?;
    let patch = numeric_prefix(parts.next()?)?;
    Some([major, minor, patch])
}

fn numeric_component(value: &str) -> Option<u32> {
    (!value.is_empty() && value.chars().all(|character| character.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

fn numeric_prefix(value: &str) -> Option<u32> {
    let digits = value
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn minimum_version(spec: &str) -> Option<[u32; 3]> {
    let index = spec.find(">=")? + 2;
    let value = spec[index..].trim_start();
    let mut parts = value.split('.');
    let major = numeric_prefix(parts.next()?)?;
    let minor = numeric_prefix(parts.next()?)?;
    let patch = parts.next().and_then(numeric_prefix).unwrap_or(0);
    Some([major, minor, patch])
}

fn version_text(version: [u32; 3]) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        checks::test_support::{check, context, metadata, site_packages},
        vocabulary::{Severity, Status},
    };

    fn solstone_metadata(site_packages: &Path, requires_python: &str) {
        metadata(
            site_packages,
            "solstone-1.2.3.dist-info",
            SOLSTONE,
            "1.2.3",
            Some(requires_python),
        );
    }

    #[test]
    fn uses_pyvenv_cfg_before_the_staged_python_directory_fallback() {
        let exact = context();
        let exact_site = site_packages(&exact, "python3.12");
        solstone_metadata(&exact_site, ">=3.12");
        fs::write(
            exact
                .install_bin_dir
                .parent()
                .expect("staged prefix")
                .join("pyvenv.cfg"),
            "version = 3.12.4\n",
        )
        .expect("write pyvenv config");
        let check = check("python_version", Severity::Blocker);
        let result = run(&exact, check).unwrap();
        assert_eq!(result.status, Status::Ok);
        assert_eq!(result.detail, "python 3.12.4 satisfies >=3.12");

        let fallback = context();
        let fallback_site = site_packages(&fallback, "python3.12");
        solstone_metadata(&fallback_site, ">=3.12");
        let result = run(&fallback, check).unwrap();
        assert_eq!(result.status, Status::Ok);
        assert!(result.detail.contains("python 3.12.0"));
        assert!(
            result
                .detail
                .contains("patch version unavailable without pyvenv.cfg")
        );

        let malformed_config = context();
        let malformed_site = site_packages(&malformed_config, "python3.12");
        solstone_metadata(&malformed_site, ">=3.12");
        fs::write(
            malformed_config
                .install_bin_dir
                .parent()
                .expect("staged prefix")
                .join("pyvenv.cfg"),
            "version = 3x.12.4\n",
        )
        .expect("write malformed pyvenv config");
        let result = run(&malformed_config, check).unwrap();
        assert_eq!(result.status, Status::Ok);
        assert!(
            result
                .detail
                .contains("patch version unavailable without pyvenv.cfg")
        );

        let incompatible = context();
        let incompatible_site = site_packages(&incompatible, "python3.10");
        solstone_metadata(&incompatible_site, ">=3.11");
        fs::write(
            incompatible
                .install_bin_dir
                .parent()
                .expect("staged prefix")
                .join("pyvenv.cfg"),
            "version = 3.10.0\n",
        )
        .expect("write incompatible pyvenv config");
        assert_eq!(run(&incompatible, check).unwrap().status, Status::Fail);

        let unresolved = context();
        let result = run(&unresolved, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert!(result.detail.contains("could not resolve site-packages"));
    }
}
