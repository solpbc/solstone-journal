// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackRefusal {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for RollbackRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RollbackRefusal {}

pub fn compare_semver_3part(a: &str, b: &str) -> Option<Ordering> {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();
    if a_parts.len() != 3 || b_parts.len() != 3 {
        return None;
    }

    for (p_a, p_b) in a_parts.iter().zip(b_parts.iter()) {
        let n_a = p_a.parse::<u64>().ok()?;
        let n_b = p_b.parse::<u64>().ok()?;
        match n_a.cmp(&n_b) {
            Ordering::Equal => continue,
            other => return Some(other),
        }
    }
    Some(Ordering::Equal)
}

pub fn version_is_older(left: &str, right: &str) -> bool {
    compare_semver_3part(left, right) == Some(Ordering::Less)
}

#[derive(Debug, Clone)]
pub struct ReleaseMetadata {
    pub version: String,
    pub epoch: String,
    pub retention_window: u32,
    pub reader_min: Option<String>,
    pub reader_max: Option<String>,
}

pub fn parse_release_metadata(release_file: &Path) -> Option<ReleaseMetadata> {
    let content = fs::read_to_string(release_file).ok()?;
    let mut version = None;
    let mut epoch = None;
    let mut window = None;
    let mut r_min = None;
    let mut r_max = None;

    for line in content.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "version" => version = Some(v.trim().to_owned()),
                "upgrade_epoch" => epoch = Some(v.trim().to_owned()),
                "retention_window" => window = v.trim().parse::<u32>().ok(),
                "state_reader_min" => r_min = Some(v.trim().to_owned()),
                "state_reader_max" => r_max = Some(v.trim().to_owned()),
                _ => {}
            }
        }
    }

    Some(ReleaseMetadata {
        version: version?,
        epoch: epoch?,
        retention_window: window?,
        reader_min: r_min,
        reader_max: r_max,
    })
}

#[derive(Debug, Clone)]
struct RetainedCandidate {
    version: String,
    dir_path: PathBuf,
    meta: ReleaseMetadata,
}

fn collect_retained_candidates(prefix: &Path, current_dest: &Path) -> Vec<RetainedCandidate> {
    let versions_dir = prefix.join("versions");
    let mut candidates = Vec::new();

    let mut entries = match fs::read_dir(&versions_dir) {
        Ok(e) => match e.collect::<Result<Vec<_>, _>>() {
            Ok(entries) => entries,
            Err(_) => return candidates,
        },
        Err(_) => return candidates,
    };
    // Native retention keeps current plus the first two visible entries by
    // filesystem modification time. Invalid entries consume a slot too.
    entries.retain(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
    entries.sort_by(|a, b| {
        let a_time = fs::symlink_metadata(a.path())
            .and_then(|m| m.modified())
            .ok();
        let b_time = fs::symlink_metadata(b.path())
            .and_then(|m| m.modified())
            .ok();
        b_time
            .cmp(&a_time)
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });

    let mut retained = 0;
    for entry in entries {
        let path = entry.path();
        if path.file_name() == current_dest.file_name() {
            continue;
        }

        retained += 1;
        if retained > 2 {
            break;
        }
        if !path.is_dir() {
            continue;
        }

        if let Ok(sym) = fs::symlink_metadata(&path)
            && sym.file_type().is_symlink()
        {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        let sha_file = path.join(".archive-sha256");
        let release_file = path.join(".release");
        if !sha_file.is_file() || !release_file.is_file() {
            continue;
        }

        let digest = match fs::read_to_string(&sha_file) {
            Ok(s) => s.trim().to_owned(),
            Err(_) => continue,
        };
        if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        let digest12 = &digest[..12];
        let suffix = format!("-{digest12}");
        let ver_from_name = match name.strip_suffix(&suffix) {
            Some(v) => v,
            None => continue,
        };

        let meta = match parse_release_metadata(&release_file) {
            Some(m) => m,
            None => continue,
        };

        if meta.version != ver_from_name {
            continue;
        }

        candidates.push(RetainedCandidate {
            version: meta.version.clone(),
            dir_path: path,
            meta,
        });
    }

    candidates
}

pub fn check_target_admissibility(
    prefix: &Path,
    installed_version: &str,
    target_version: &str,
) -> Result<PathBuf, RollbackRefusal> {
    check_target_admissibility_in(
        solstone_core_cli_copy::ENGLISH,
        prefix,
        installed_version,
        target_version,
    )
}

pub fn check_target_admissibility_in(
    locale: &str,
    prefix: &Path,
    installed_version: &str,
    target_version: &str,
) -> Result<PathBuf, RollbackRefusal> {
    let current_link = prefix.join("current");
    let current_dest = fs::read_link(&current_link)
        .map(|p| if p.is_absolute() { p } else { prefix.join(p) })
        .map_err(|_| RollbackRefusal {
            code: "downgrade-window".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.link",
                &[],
            ),
        })?;

    let installed_meta =
        parse_release_metadata(&current_dest.join(".release")).ok_or_else(|| RollbackRefusal {
            code: "compatibility-unknown".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.metadata",
                &[],
            ),
        })?;

    let receipt = crate::ownership::parse_install_receipt(&prefix.join("install-receipt"))
        .unwrap_or_default();
    if receipt.get("journal_state").map(String::as_str) != Some("present") {
        return Err(RollbackRefusal {
            code: "compatibility-unknown".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.state",
                &[],
            ),
        });
    }
    let (Some(r_min), Some(r_max)) = (&installed_meta.reader_min, &installed_meta.reader_max)
    else {
        return Err(RollbackRefusal {
            code: "compatibility-unknown".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.range.absent",
                &[],
            ),
        });
    };
    if installed_meta.version != installed_version
        || !matches!(
            compare_semver_3part(target_version, r_min),
            Some(Ordering::Equal | Ordering::Greater)
        )
        || !matches!(
            compare_semver_3part(target_version, r_max),
            Some(Ordering::Equal | Ordering::Less)
        )
    {
        return Err(RollbackRefusal {
            code: "compatibility-unknown".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.range.outside",
                &[],
            ),
        });
    }
    let candidates = collect_retained_candidates(prefix, &current_dest);

    // Look for target within retention window (max 3 retained)
    let window_candidates = if candidates.len() > 3 {
        &candidates[..3]
    } else {
        &candidates[..]
    };

    let target_cand = window_candidates
        .iter()
        .find(|c| c.version == target_version);

    let target_cand = match target_cand {
        Some(c) => c,
        None => {
            return Err(RollbackRefusal {
                code: "downgrade-window".to_owned(),
                message: solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.rollback.retained",
                    &[],
                ),
            });
        }
    };

    if target_cand.meta.epoch != "journal-v2" || installed_meta.epoch != "journal-v2" {
        return Err(RollbackRefusal {
            code: "downgrade-epoch".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.epoch",
                &[],
            ),
        });
    }

    if target_cand.meta.retention_window != installed_meta.retention_window
        || target_cand.meta.retention_window != 3
    {
        return Err(RollbackRefusal {
            code: "retention-policy-unsupported".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.policy",
                &[],
            ),
        });
    }

    if !version_is_older(target_version, installed_version) {
        return Err(RollbackRefusal {
            code: "compatibility-unknown".to_owned(),
            message: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.sol.update.rollback.version",
                [
                    ("target_version", &(target_version)),
                    ("installed_version", &(installed_version)),
                ],
            ),
        });
    }

    Ok(target_cand.dir_path.clone())
}

pub fn select_newest_admissible_rollback(
    prefix: &Path,
    installed_version: &str,
) -> Result<(String, PathBuf), RollbackRefusal> {
    select_newest_admissible_rollback_in(solstone_core_cli_copy::ENGLISH, prefix, installed_version)
}

pub fn select_newest_admissible_rollback_in(
    locale: &str,
    prefix: &Path,
    installed_version: &str,
) -> Result<(String, PathBuf), RollbackRefusal> {
    let current_link = prefix.join("current");
    let current_dest = fs::read_link(&current_link)
        .map(|p| if p.is_absolute() { p } else { prefix.join(p) })
        .map_err(|_| RollbackRefusal {
            code: "downgrade-window".to_owned(),
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.rollback.link",
                &[],
            ),
        })?;

    let candidates = collect_retained_candidates(prefix, &current_dest);
    let mut last_error = None;
    for candidate in candidates {
        if !version_is_older(&candidate.version, installed_version) {
            continue;
        }
        match check_target_admissibility_in(locale, prefix, installed_version, &candidate.version) {
            Ok(path) => return Ok((candidate.version, path)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or(RollbackRefusal {
        code: "downgrade-window".to_owned(),
        message: solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.sol.update.rollback.no.older",
            &[],
        ),
    }))
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod tests {
    use super::*;

    fn setup_installed(prefix: &Path, ver: &str, min: &str, epoch: &str, window: u32) -> PathBuf {
        let digest = "1111111111111111111111111111111111111111111111111111111111111111";
        let entry = format!("{ver}-{}", &digest[..12]);
        let dir = prefix.join("versions").join(&entry);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join(".archive-sha256"), digest).unwrap();
        let release = format!(
            "product=solstone-journal\nversion={ver}\nupgrade_epoch={epoch}\nretention_window={window}\nstate_reader_min={min}\nstate_reader_max={ver}\n"
        );
        fs::write(dir.join(".release"), release).unwrap();
        fs::write(prefix.join("install-receipt"), "journal_state=present\n").unwrap();

        #[cfg(unix)]
        {
            let current = prefix.join("current");
            let _ = fs::remove_file(&current);
            std::os::unix::fs::symlink(format!("versions/{entry}"), &current).unwrap();
        }
        dir
    }

    fn add_retained(
        prefix: &Path,
        ver: &str,
        epoch: &str,
        window: u32,
        digest_byte: u8,
    ) -> PathBuf {
        let digest = format!("{:02x}", digest_byte).repeat(32);
        let entry = format!("{ver}-{}", &digest[..12]);
        let dir = prefix.join("versions").join(&entry);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join(".archive-sha256"), &digest).unwrap();
        let release = format!(
            "product=solstone-journal\nversion={ver}\nupgrade_epoch={epoch}\nretention_window={window}\nstate_reader_min=1.0.0\nstate_reader_max={ver}\n"
        );
        fs::write(dir.join(".release"), release).unwrap();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let timestamp = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        fs::File::open(&dir)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(timestamp))
            .unwrap();
        dir
    }

    #[test]
    fn downgrade_refuses_target_outside_installed_reader_range() {
        let temp = tempfile::tempdir().unwrap();
        setup_installed(temp.path(), "2.0.48", "2.0.45", "journal-v2", 3);
        add_retained(temp.path(), "2.0.40", "journal-v2", 3, 2);

        let res = check_target_admissibility(temp.path(), "2.0.48", "2.0.40");
        let err = res.unwrap_err();
        assert_eq!(err.code, "compatibility-unknown");
    }

    #[test]
    fn downgrade_refuses_crossed_upgrade_epoch() {
        let temp = tempfile::tempdir().unwrap();
        setup_installed(temp.path(), "2.0.48", "1.0.0", "journal-v3", 3);
        add_retained(temp.path(), "2.0.47", "journal-v2", 3, 2);

        let res = check_target_admissibility(temp.path(), "2.0.48", "2.0.47");
        let err = res.unwrap_err();
        assert_eq!(err.code, "downgrade-epoch");
    }

    #[test]
    fn downgrade_refuses_unsupported_retention_window() {
        let temp = tempfile::tempdir().unwrap();
        setup_installed(temp.path(), "2.0.48", "1.0.0", "journal-v2", 3);
        add_retained(temp.path(), "2.0.47", "journal-v2", 5, 2); // window 5 != 3

        let res = check_target_admissibility(temp.path(), "2.0.48", "2.0.47");
        let err = res.unwrap_err();
        assert_eq!(err.code, "retention-policy-unsupported");
    }

    #[test]
    fn downgrade_refuses_unretained_target_directory() {
        let temp = tempfile::tempdir().unwrap();
        setup_installed(temp.path(), "2.0.48", "1.0.0", "journal-v2", 3);

        let res = check_target_admissibility(temp.path(), "2.0.48", "2.0.47");
        let err = res.unwrap_err();
        assert_eq!(err.code, "downgrade-window");
    }

    #[test]
    fn downgrade_refuses_target_outside_three_directory_window() {
        let temp = tempfile::tempdir().unwrap();
        setup_installed(temp.path(), "2.0.48", "1.0.0", "journal-v2", 3);
        add_retained(temp.path(), "2.0.44", "journal-v2", 3, 5);
        add_retained(temp.path(), "2.0.45", "journal-v2", 3, 4);
        add_retained(temp.path(), "2.0.46", "journal-v2", 3, 3);
        add_retained(temp.path(), "2.0.47", "journal-v2", 3, 2); // 4th retained version

        // 2.0.44 is outside the 3 newest retained builds (47, 46, 45)
        let res = check_target_admissibility(temp.path(), "2.0.48", "2.0.44");
        let err = res.unwrap_err();
        assert_eq!(err.code, "downgrade-window");
    }

    #[test]
    fn rollback_selects_newest_admissible_retained_directory() {
        let temp = tempfile::tempdir().unwrap();
        setup_installed(temp.path(), "2.0.48", "1.0.0", "journal-v2", 3);
        add_retained(temp.path(), "2.0.45", "journal-v2", 3, 4);
        add_retained(temp.path(), "2.0.46", "journal-v2", 3, 3);
        add_retained(temp.path(), "2.0.47", "journal-v2", 3, 2);

        let (ver, _dir) = select_newest_admissible_rollback(temp.path(), "2.0.48").unwrap();
        assert_eq!(ver, "2.0.47");
    }

    #[test]
    fn retained_digest_does_not_create_a_platform_revision() {
        let temp = tempfile::tempdir().unwrap();
        setup_installed(temp.path(), "2.0.48", "1.0.0", "journal-v2", 3);
        let retained = add_retained(temp.path(), "2.0.47", "journal-v2", 3, 7);

        let entry = retained.file_name().unwrap().to_str().unwrap();
        assert!(entry.starts_with("2.0.47-"));
        // The version itself remains pure 3-part
        let (ver, _) = select_newest_admissible_rollback(temp.path(), "2.0.48").unwrap();
        assert_eq!(ver, "2.0.47");
        assert!(!ver.contains('-'));
    }
}
