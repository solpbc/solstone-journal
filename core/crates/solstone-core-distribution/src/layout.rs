// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use crate::inventory::{OS_LINUX, OS_MACOS, OS_WINDOWS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsLayoutRole {
    Runtime,
    Deps { name: String },
    Models,
    Licenses { name: String },
    Provenance,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutError {
    pub message: String,
}

impl LayoutError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LayoutError {}

pub fn admit_dest(os: &str, dest: &str) -> Result<(), LayoutError> {
    match os {
        OS_LINUX | OS_MACOS => Ok(()),
        OS_WINDOWS => windows_dest_role(dest).map(|_| ()),
        other => Err(LayoutError::new(format!("unexpected os {other}"))),
    }
}

pub fn windows_dest_role(dest: &str) -> Result<WindowsLayoutRole, LayoutError> {
    if dest.is_empty() {
        return Err(LayoutError::new("empty dest"));
    }
    if dest.starts_with('/') {
        return Err(LayoutError::new("absolute dest"));
    }
    if dest.contains('\\') {
        return Err(LayoutError::new("non-POSIX separator"));
    }
    let parts = dest.split('/').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err(LayoutError::new("non-canonical dest"));
    }
    let Some((first, rest)) = parts.split_first() else {
        return Err(LayoutError::new("empty dest"));
    };
    match *first {
        "bin" => require_file_under_root(rest, WindowsLayoutRole::Runtime),
        "lib" if rest.first() == Some(&"solstone_journal_models") => {
            require_file_under_root(&rest[1..], WindowsLayoutRole::Models)
        }
        "lib" => named_role(rest, |name| WindowsLayoutRole::Deps { name }),
        "share" if rest.first() == Some(&"licenses") => {
            named_role(&rest[1..], |name| WindowsLayoutRole::Licenses { name })
        }
        "share" if rest.first() == Some(&"provenance") => {
            require_file_under_root(&rest[1..], WindowsLayoutRole::Provenance)
        }
        "share" => require_file_under_root(rest, WindowsLayoutRole::Content),
        other => Err(LayoutError::new(format!(
            "unknown windows dest root {other}"
        ))),
    }
}

fn require_file_under_root(
    rest: &[&str],
    role: WindowsLayoutRole,
) -> Result<WindowsLayoutRole, LayoutError> {
    if rest.is_empty() {
        return Err(LayoutError::new("bare windows dest root"));
    }
    Ok(role)
}

fn named_role(
    rest: &[&str],
    role: impl FnOnce(String) -> WindowsLayoutRole,
) -> Result<WindowsLayoutRole, LayoutError> {
    let Some(name) = rest.first().copied().filter(|name| !name.is_empty()) else {
        return Err(LayoutError::new("missing windows dest name"));
    };
    if rest.len() < 2 {
        return Err(LayoutError::new(
            "missing windows dest file under named root",
        ));
    }
    Ok(role(name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_and_macos_dests_are_unrestricted() {
        admit_dest(OS_LINUX, "bin/solstone-core").expect("linux");
        admit_dest(OS_MACOS, "lib/solstone-core-pdf/libpdfium.dylib").expect("macos");
        admit_dest(OS_LINUX, "share/LICENSE").expect("payload prefix");
    }

    #[test]
    fn windows_roles_accept_nested_files() {
        assert_eq!(
            windows_dest_role("bin/test-fixture-bin.exe").unwrap(),
            WindowsLayoutRole::Runtime
        );
        assert_eq!(
            windows_dest_role("lib/solstone-core-speakers-analyze/onnxruntime.dll").unwrap(),
            WindowsLayoutRole::Deps {
                name: "solstone-core-speakers-analyze".to_owned(),
            }
        );
        assert_eq!(
            windows_dest_role("lib/solstone_journal_models/assets/wespeaker.onnx").unwrap(),
            WindowsLayoutRole::Models
        );
        assert_eq!(
            windows_dest_role("share/licenses/solstone/LICENSE").unwrap(),
            WindowsLayoutRole::Licenses {
                name: "solstone".to_owned(),
            }
        );
        assert_eq!(
            windows_dest_role("share/provenance/receipt.json").unwrap(),
            WindowsLayoutRole::Provenance
        );
    }

    #[test]
    fn windows_dests_refuse_the_named_rejection_rules() {
        for dest in [
            "",
            "/bin/a.exe",
            "bin\\a.exe",
            "bin/./a.exe",
            "bin/../a.exe",
            "bin",
            "lib",
            "share",
            "share/provenance",
            "share/licenses",
            "lib/onnxruntime",
            "share/licenses/solstone",
            "runtime/a.exe",
            "deps/onnxruntime/a.dll",
            "models/model.onnx",
            "licenses/solstone/LICENSE",
            "provenance/receipt.json",
        ] {
            assert!(windows_dest_role(dest).is_err(), "{dest} should be refused");
        }
    }
}
