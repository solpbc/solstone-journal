// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value};

use crate::args::InventoryOptions;

pub(crate) fn run(
    _talent_root: &Path,
    _apps_root: &Path,
    _journal_root: &Path,
    options: &InventoryOptions,
) -> Result<String, String> {
    if options.json {
        let mut root = Map::new();
        root.insert("talents".to_owned(), Value::Array(Vec::new()));
        root.insert("tiers".to_owned(), Value::Object(Map::new()));
        Ok(format!(
            "{}\n",
            solstone_core_format::json_compact_ascii(&Value::Object(root))
        ))
    } else {
        Ok("No cogitate talents found.\n".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("root");
        for directory in ["talent", "apps", "config"] {
            fs::create_dir_all(root.path().join(directory)).expect("directory");
        }
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"name":"Sol"}}"#,
        )
        .expect("config");
        root
    }

    fn inventory(root: &tempfile::TempDir, json: bool) -> String {
        run(
            &root.path().join("talent"),
            &root.path().join("apps"),
            root.path(),
            &InventoryOptions { json },
        )
        .expect("inventory")
    }

    #[test]
    fn empty_table_has_no_tier_section() {
        let root = root();
        assert_eq!(inventory(&root, false), "No cogitate talents found.\n");
        let json = inventory(&root, true);
        let parsed: Value = serde_json::from_str(&json).expect("json");
        assert_eq!(parsed["talents"], json!([]));
    }
}
