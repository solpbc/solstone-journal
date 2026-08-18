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
    let inherited_path = inherited_path.unwrap_or(DEFAULT_PATH);
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
    fn missing_path_uses_default_fallback() {
        let environment = build_service_environment("/home/sol", None, "/runtime");
        assert_eq!(environment["PATH"], "/runtime:/usr/local/bin:/usr/bin:/bin");
        assert!(!environment.contains_key("PYTHONUNBUFFERED"));
    }

    #[test]
    fn present_empty_path_retains_its_empty_component() {
        let environment = build_service_environment("/home/sol", Some(""), "/runtime");
        assert_eq!(environment["PATH"], "/runtime:");
    }
}
