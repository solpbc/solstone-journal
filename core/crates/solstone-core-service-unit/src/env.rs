// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Build the exact environment carried by an installed Solstone service.
pub fn build_service_environment(
    home: &str,
    inherited_path: Option<&str>,
    runtime_executable_dir: &str,
) -> BTreeMap<String, String> {
    let inherited_path = inherited_path
        .filter(|path| !path.is_empty())
        .unwrap_or(DEFAULT_PATH);
    let mut seen = BTreeSet::new();
    let mut parts = Vec::new();
    for part in std::iter::once(runtime_executable_dir).chain(inherited_path.split(':')) {
        if seen.insert(part) {
            parts.push(part);
        }
    }

    BTreeMap::from([
        ("HOME".to_owned(), home.to_owned()),
        ("PATH".to_owned(), parts.join(":")),
        ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
    ])
}

#[cfg(test)]
mod tests {
    use super::build_service_environment;

    #[test]
    fn prepends_and_deduplicates_path_components() {
        let environment = build_service_environment(
            "/home/sol",
            Some("/usr/bin:/runtime:/usr/bin:/bin"),
            "/runtime",
        );
        assert_eq!(environment["PATH"], "/runtime:/usr/bin:/bin");
    }

    #[test]
    fn empty_or_missing_path_uses_python_fallback() {
        for inherited in [None, Some("")] {
            let environment = build_service_environment("/home/sol", inherited, "/runtime");
            assert_eq!(environment["PATH"], "/runtime:/usr/local/bin:/usr/bin:/bin");
        }
    }
}
