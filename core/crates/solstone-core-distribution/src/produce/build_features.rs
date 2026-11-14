// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cargo invocations are feature-unification boundaries, even within one target.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use super::{ProduceError, bins_for_resolved_lane};
use crate::inventory::{Inventory, OS_WINDOWS};

pub(super) fn requires_linked_ort(bin: &str) -> bool {
    matches!(
        bin,
        "solstone-core-speakers-analyze" | "solstone-core-vad-analyze"
    )
}

/// Describe opens the OCR runtime explicitly; speech links a different runtime.
/// Keep their Cargo feature closures separate on both GNU and Apple lanes.
pub(super) fn batches(bins: &[(String, String)]) -> Vec<Vec<(String, String)>> {
    let (describe, other): (Vec<_>, Vec<_>) = bins
        .iter()
        .cloned()
        .partition(|(package, _)| package == "solstone-core-describe");
    [other, describe]
        .into_iter()
        .filter(|bins| !bins.is_empty())
        .collect()
}

/// Resolve precisely the production dependency edges for each producer batch.
/// This needs no target toolchain, runtime, linker, or release compilation.
pub fn check(checkout: &Path, inventory: &Inventory) -> Result<usize, ProduceError> {
    let mut checked = 0;
    for target in &inventory.target {
        if target.os == OS_WINDOWS {
            continue;
        }
        let lanes: BTreeSet<_> = inventory
            .entry
            .iter()
            .filter_map(|entry| {
                if let crate::inventory::Entry::Bin { lane, targets, .. } = entry {
                    targets
                        .contains(&target.id)
                        .then_some(target.lane_for(lane))
                } else {
                    None
                }
            })
            .collect();
        for lane in lanes {
            for bins in batches(&bins_for_resolved_lane(inventory, target, lane)) {
                checked += usize::from(check_batch(checkout, target.triple_for_lane(lane), &bins)?);
            }
        }
    }
    if checked == 0 {
        return Err(ProduceError::new("no ORT-linked producer batches found"));
    }
    Ok(checked)
}

pub(super) fn check_batch(
    checkout: &Path,
    triple: &str,
    bins: &[(String, String)],
) -> Result<bool, ProduceError> {
    if !bins.iter().any(|(_, bin)| requires_linked_ort(bin)) {
        return Ok(false);
    }
    let mut command = Command::new("cargo");
    command.current_dir(checkout).args([
        "tree",
        "--manifest-path",
        "core/Cargo.toml",
        "--locked",
        "--offline",
        "--target",
        triple,
        "--edges",
        "normal,build",
        "--invert",
        "ort-sys",
        "--depth",
        "0",
        "--prefix",
        "none",
        "--format",
        "{p}\t{f}",
    ]);
    for (package, _) in bins {
        command.args(["-p", package]);
    }
    if bins
        .iter()
        .any(|(package, bin)| package == "solstone-core" && bin == "solstone-core")
    {
        command.args(super::shipped_core_feature_args());
    }
    let output = command
        .output()
        .map_err(|error| ProduceError::new(format!("cargo tree: {error}")))?;
    if !output.status.success() {
        return Err(ProduceError::new(format!(
            "ORT feature preflight for {triple} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    admit_linked_features(&String::from_utf8_lossy(&output.stdout)).map_err(|error| {
        ProduceError::new(format!("ORT feature preflight for {triple}: {error}"))
    })?;
    Ok(true)
}

fn admit_linked_features(output: &str) -> Result<(), ProduceError> {
    let lines: Vec<_> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let Some((package, features)) = (lines.len() == 1)
        .then(|| lines[0].split_once('\t'))
        .flatten()
    else {
        return Err(ProduceError::new(
            "expected exactly one resolved ort-sys feature row",
        ));
    };
    if !package.starts_with("ort-sys v") {
        return Err(ProduceError::new("resolved feature row is not ort-sys"));
    }
    if features
        .split(',')
        .any(|feature| feature == "disable-linking")
    {
        return Err(ProduceError::new(
            "speech helpers require linked ORT, but ort-sys/disable-linking is enabled (ort/load-dynamic must use a separate Cargo invocation)",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured with check_batch's cargo tree arguments on Linux, selecting
    // speakers-analyze alone, then with describe in the same invocation.
    const LINKED: &str =
        "ort-sys v2.0.0-rc.12\tapi-17,api-18,api-19,api-20,api-21,api-22,api-23,api-24,std\n";
    const DYNAMIC: &str = "ort-sys v2.0.0-rc.12\tapi-17,api-18,api-19,api-20,api-21,api-22,api-23,api-24,disable-linking,std\n";

    #[test]
    fn refuses_unified_dynamic_loading_and_missing_resolution() {
        admit_linked_features(LINKED).unwrap();
        assert!(admit_linked_features(DYNAMIC).is_err());
        assert!(admit_linked_features("").is_err());
        assert!(admit_linked_features(&format!("{LINKED}{LINKED}")).is_err());
        assert!(admit_linked_features("other v1.0.0\tstd\n").is_err());
    }

    #[test]
    fn separates_dynamic_describe_from_both_linked_helpers() {
        let bins: Vec<_> = [
            "solstone-core-describe",
            "solstone-core-speakers-analyze",
            "solstone-core-vad-analyze",
        ]
        .map(|bin| (bin.to_owned(), bin.to_owned()))
        .into();
        let planned = batches(&bins);
        assert_eq!(planned.iter().map(Vec::len).sum::<usize>(), bins.len());
        for batch in planned {
            if batch.iter().any(|(_, bin)| requires_linked_ort(bin)) {
                assert!(
                    !batch
                        .iter()
                        .any(|(package, _)| package == "solstone-core-describe")
                );
            }
        }
        assert!(batches(&[]).is_empty());
    }
}
