// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host fit checks for Parakeet artifact installation.

use std::path::{Path, PathBuf};

use nix::sys::statvfs::statvfs;

use super::pins;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitSeverity {
    Ok,
    Warning,
    Blocked,
    Unknown,
}

impl FitSeverity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitCheck {
    pub name: &'static str,
    pub severity: FitSeverity,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitReport {
    pub artifact: &'static str,
    pub checks: Vec<FitCheck>,
}

impl FitReport {
    pub fn overall(&self) -> FitSeverity {
        if self
            .checks
            .iter()
            .any(|check| check.severity == FitSeverity::Blocked)
        {
            FitSeverity::Blocked
        } else if self
            .checks
            .iter()
            .any(|check| matches!(check.severity, FitSeverity::Warning | FitSeverity::Unknown))
        {
            FitSeverity::Warning
        } else {
            FitSeverity::Ok
        }
    }
}

pub fn render_fit_report(report: &FitReport) -> String {
    let mut lines = vec![format!(
        "{} fit check: {}",
        report.artifact,
        report.overall().as_str()
    )];
    lines.extend(report.checks.iter().map(|check| {
        format!(
            "[{}] {}: {}",
            check.severity.as_str(),
            check.name,
            check.detail
        )
    }));
    lines.join("\n")
}

pub fn build_parakeet_fit_report(journal: &Path, os_name: &str, arch: &str) -> FitReport {
    build_parakeet_fit_report_with_free_bytes(
        journal,
        os_name,
        arch,
        free_bytes(&pins::parakeet_cache_root(journal)),
    )
}

pub fn build_parakeet_fit_report_with_free_bytes(
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    let platform = match pins::parakeet_artifact_key(os_name, arch) {
        Ok(key) => FitCheck {
            name: "platform",
            severity: FitSeverity::Ok,
            detail: format!("pinned parakeet.cpp artifacts are available for {key}"),
        },
        Err(error) => FitCheck {
            name: "platform",
            severity: FitSeverity::Blocked,
            detail: error.to_string(),
        },
    };
    let cache = pins::parakeet_cache_root(journal);
    let disk = disk_check(
        &cache,
        available,
        &[("parakeet GGUF model", pins::PARAKEET_MODEL.4)],
        &[
            "parakeet CPU server tarball",
            "parakeet Vulkan server tarball",
        ],
    );
    FitReport {
        artifact: "parakeet.cpp artifacts",
        checks: vec![platform, disk],
    }
}

pub fn build_rfdetr_fit_report(journal: &Path, os_name: &str, arch: &str) -> FitReport {
    build_rfdetr_fit_report_with_free_bytes(
        journal,
        os_name,
        arch,
        free_bytes(&journal.join("cache/providers/rfdetr")),
    )
}

pub fn build_rfdetr_fit_report_with_free_bytes(
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    let platform = if os_name == "linux" && matches!(arch, "amd64" | "x64" | "x86_64") {
        FitCheck {
            name: "platform",
            severity: FitSeverity::Ok,
            detail: "pinned rf-detr.cpp artifacts are available for x86_64-linux".to_owned(),
        }
    } else {
        FitCheck {
            name: "platform",
            severity: FitSeverity::Blocked,
            detail: format!("rf-detr.cpp requires x86_64 Linux, got {os_name}/{arch}"),
        }
    };
    let cache = journal.join("cache/providers/rfdetr");
    let disk = disk_check(
        &cache,
        available,
        &[
            ("rf-detr GGUF model", 63_439_488),
            ("rf-detr CLI binary", 1_048_576),
        ],
        &[],
    );
    FitReport {
        artifact: "rf-detr.cpp artifacts",
        checks: vec![platform, disk],
    }
}

fn disk_check(
    cache: &Path,
    available: Result<u64, String>,
    known: &[(&str, u64)],
    unknown: &[&str],
) -> FitCheck {
    let required = known.iter().map(|(_, size)| size).sum::<u64>();
    let known_names = known
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    let unknown = if unknown.is_empty() {
        String::new()
    } else {
        format!("unknown download size for {}", unknown.join(", "))
    };
    match available {
        Err(error) => FitCheck {
            name: "disk",
            severity: FitSeverity::Unknown,
            detail: if unknown.is_empty() {
                format!(
                    "available disk space could not be verified at {}: {error}",
                    cache.display()
                )
            } else {
                format!(
                    "available disk space could not be verified at {}: {error}; {unknown}",
                    cache.display()
                )
            },
        },
        Ok(available) if available < required => {
            let mut detail = format!(
                "insufficient disk space for known downloads (need {} GB, have {} GB free){}{}",
                gb_label(required),
                gb_label(available),
                if known_names.is_empty() { "" } else { ": " },
                known_names,
            );
            if !unknown.is_empty() {
                detail.push_str(&format!("; {unknown}"));
            }
            FitCheck {
                name: "disk",
                severity: FitSeverity::Blocked,
                detail,
            }
        }
        Ok(available) => {
            let detail = if unknown.is_empty() {
                format!(
                    "{} GB free for {} GB known downloads",
                    gb_label(available),
                    gb_label(required)
                )
            } else {
                format!(
                    "{} GB free; known downloads need {} GB; {unknown}",
                    gb_label(available),
                    gb_label(required)
                )
            };
            FitCheck {
                name: "disk",
                severity: if unknown.is_empty() {
                    FitSeverity::Ok
                } else {
                    FitSeverity::Warning
                },
                detail,
            }
        }
    }
}

pub fn free_bytes(target: &Path) -> Result<u64, String> {
    let usage_root = nearest_existing_ancestor(target);
    let stats = statvfs(&usage_root).map_err(|error| error.to_string())?;
    let blocks = stats.blocks_available() as u64;
    let fragment_size = stats.fragment_size() as u64;
    blocks
        .checked_mul(fragment_size)
        .ok_or_else(|| "available disk space overflow".to_owned())
}

fn nearest_existing_ancestor(target: &Path) -> PathBuf {
    let mut current = target.to_path_buf();
    while !current.exists() && current != current.parent().unwrap_or(Path::new("")) {
        if !current.pop() {
            break;
        }
    }
    current
}

fn gb_label(bytes: u64) -> String {
    let value = ((bytes as f64 / 1024_f64.powi(3)) * 10.0).round() / 10.0;
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parakeet_report_matches_the_blocked_disk_template() {
        let report = build_parakeet_fit_report_with_free_bytes(
            Path::new("/journal"),
            "linux",
            "arm64",
            Ok(1),
        );
        assert_eq!(report.overall(), FitSeverity::Blocked);
        assert_eq!(
            render_fit_report(&report),
            "parakeet.cpp artifacts fit check: blocked\n[ok] platform: pinned parakeet.cpp artifacts are available for aarch64-unknown-linux-gnu\n[blocked] disk: insufficient disk space for known downloads (need 0.9 GB, have 0 GB free): parakeet GGUF model; unknown download size for parakeet CPU server tarball, parakeet Vulkan server tarball"
        );
    }
}
