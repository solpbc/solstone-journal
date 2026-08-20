// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

/// The repository directory `payload.txt`'s paths are rooted in, read from the
/// distribution inventory rather than restated here.
///
/// The payload's repository location and its declaration have to agree, and a
/// second copy of the root in this crate is a second place that can disagree
/// with the first. Reading the key means changing it in the inventory moves
/// this contract with it — and pointing it at a directory that does not exist
/// turns the whole tracked set empty, which is what the emptiness assertion in
/// `payload_txt_matches_git_tracked_inventory` exists to catch.
pub fn payload_src_root(inventory_toml: &str) -> Result<String, String> {
    let document = inventory_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("parse distribution inventory: {error}"))?;
    document
        .get("payload_src_root")
        .and_then(toml_edit::Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "distribution inventory has no payload_src_root".to_owned())
}

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

/// Strip the payload source root from a repository-relative Git path.
///
/// `payload.txt` is `payload_src_root`-relative and `git ls-files` is
/// repository-relative, so the two are only comparable after this. A path
/// outside the root is dropped rather than passed through, so a stray file
/// cannot be mistaken for a declared one.
pub fn strip_payload_src_root(root: &str, path: &[u8]) -> Option<Vec<u8>> {
    let mut prefix = root.as_bytes().to_vec();
    prefix.push(b'/');
    path.strip_prefix(prefix.as_slice()).map(<[u8]>::to_vec)
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
    fn payload_src_root_is_read_from_the_inventory() {
        assert_eq!(
            payload_src_root("version = 1\npayload_src_root = \"core/payload\"\n")
                .expect("declared root"),
            "core/payload"
        );
        assert_eq!(
            payload_src_root("version = 1\n").expect_err("absent root fails"),
            "distribution inventory has no payload_src_root"
        );
    }

    #[test]
    fn stripping_the_source_root_drops_paths_outside_it() {
        assert_eq!(
            strip_payload_src_root(
                "core/payload",
                b"core/payload/solstone/talent/conversation.md"
            ),
            Some(b"solstone/talent/conversation.md".to_vec())
        );
        assert_eq!(
            strip_payload_src_root("core/payload", b"solstone/talent/conversation.md"),
            None
        );
        assert_eq!(
            strip_payload_src_root(
                "core/payload",
                b"core/payloadish/solstone/talent/conversation.md"
            ),
            None
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
