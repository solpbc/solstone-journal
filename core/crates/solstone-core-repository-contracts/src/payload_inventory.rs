// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

pub fn declared_paths(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

pub fn tracked_paths(output: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if !output.is_empty() && output.last() != Some(&0) {
        return Err("git ls-files output is missing its final NUL terminator".to_owned());
    }

    let mut paths = output.split(|byte| *byte == 0).peekable();
    let mut parsed = Vec::new();
    while let Some(path) = paths.next() {
        if path.is_empty() {
            if paths.peek().is_some() {
                return Err("git ls-files output contains an empty interior record".to_owned());
            }
            continue;
        }
        parsed.push(path.to_vec());
    }
    Ok(parsed)
}

pub fn is_payload_path(path: &[u8]) -> bool {
    if path.split(|byte| *byte == b'/').any(|part| part == b"..") {
        return false;
    }

    path == b"solstone/think/contract/layout.json"
        || path.starts_with(b"solstone/talent/")
        || path.starts_with(b"solstone/think/templates/")
        || path.starts_with(b"solstone/think/services/spp_attest/roots/")
        || path
            .strip_prefix(b"solstone/apps/")
            .and_then(|relative| {
                relative
                    .iter()
                    .position(|byte| *byte == b'/')
                    .map(|separator| &relative[separator + 1..])
            })
            .is_some_and(|child| child.starts_with(b"talent/"))
}

pub fn payload_set(paths: impl IntoIterator<Item = Vec<u8>>) -> BTreeSet<Vec<u8>> {
    paths
        .into_iter()
        .filter(|path| !path.ends_with(b".py"))
        .filter(|path| is_payload_path(path))
        .collect()
}

pub fn inventory_diff(
    declared: &BTreeSet<Vec<u8>>,
    tracked: &BTreeSet<Vec<u8>>,
) -> (BTreeSet<Vec<u8>>, BTreeSet<Vec<u8>>) {
    (
        tracked.difference(declared).cloned().collect(),
        declared.difference(tracked).cloned().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_covers_every_payload_family_and_rejects_near_misses() {
        let selected = payload_set([
            b"solstone/talent/example.md".to_vec(),
            b"solstone/think/templates/example.md".to_vec(),
            b"solstone/think/services/spp_attest/roots/example.pem".to_vec(),
            b"solstone/think/contract/layout.json".to_vec(),
            b"solstone/apps/example/talent/example.md".to_vec(),
            b"solstone/talent/ignored.py".to_vec(),
            b"solstone/talent/\xff.md".to_vec(),
            b"solstone/talent/../../outside.md".to_vec(),
            b"solstone/talentish/not-payload.md".to_vec(),
            b"solstone/apps/example/talent/../../../outside.md".to_vec(),
            b"solstone/apps/example/talentish/not-payload.md".to_vec(),
        ]);

        assert_eq!(
            selected,
            [
                b"solstone/apps/example/talent/example.md".to_vec(),
                b"solstone/talent/example.md".to_vec(),
                b"solstone/talent/\xff.md".to_vec(),
                b"solstone/think/contract/layout.json".to_vec(),
                b"solstone/think/services/spp_attest/roots/example.pem".to_vec(),
                b"solstone/think/templates/example.md".to_vec(),
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn tracked_output_requires_nul_termination() {
        assert_eq!(
            tracked_paths(b"solstone/talent/example.md\0").expect("valid tracked paths"),
            vec![b"solstone/talent/example.md".to_vec()]
        );
        assert_eq!(
            tracked_paths(b"solstone/talent/example.md").expect_err("missing terminator fails"),
            "git ls-files output is missing its final NUL terminator"
        );
        assert_eq!(
            tracked_paths(b"solstone/talent/example.md\0\0")
                .expect_err("empty interior record fails"),
            "git ls-files output contains an empty interior record"
        );
        assert_eq!(
            tracked_paths(b"solstone/talent/\xff.md\0").expect("non-UTF-8 path is retained"),
            vec![b"solstone/talent/\xff.md".to_vec()]
        );
    }

    #[test]
    fn inventory_diff_is_directional() {
        let declared = [b"declared-only".to_vec(), b"shared".to_vec()]
            .into_iter()
            .collect();
        let tracked = [b"shared".to_vec(), b"tracked-only".to_vec()]
            .into_iter()
            .collect();
        let (missing, unexpected) = inventory_diff(&declared, &tracked);
        assert_eq!(missing, BTreeSet::from([b"tracked-only".to_vec()]));
        assert_eq!(unexpected, BTreeSet::from([b"declared-only".to_vec()]));
    }
}
