// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::discovery::{DiscoveryError, discover_from_root};
use solstone_core_format::matcher::PatternRoot;
use solstone_core_format::paths::CHRONICLE_DIR;

use super::registry::patterns_for_root;

pub fn discover_edge_files(journal: &Path) -> Result<BTreeMap<String, PathBuf>, DiscoveryError> {
    let mut files = BTreeMap::new();
    for spec in patterns_for_root(PatternRoot::Structural) {
        discover_from_root(journal, journal, spec.pattern, &mut files)?;
    }

    let chronicle = journal.join(CHRONICLE_DIR);
    let day_root = if chronicle.is_dir() {
        chronicle.as_path()
    } else {
        journal
    };
    for spec in patterns_for_root(PatternRoot::DayRooted) {
        discover_from_root(day_root, day_root, spec.pattern, &mut files)?;
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "solstone-core-indexer-edge-discovery-{name}-{stamp}"
        ))
    }

    fn write(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create parent");
        fs::write(path, "{}\n").expect("write test file");
    }

    #[test]
    fn discovers_edge_rels_from_edge_registry_only() {
        let root = temp_root("registry");
        write(&root, "chronicle/20240101/talents/flow.md");
        write(&root, "facets/work/entities/20260304.jsonl");
        write(&root, "facets/work/activities/20260304.jsonl");
        write(&root, "facets/work/entities/alice/observations.jsonl");
        write(&root, "facets/work/events/20260304.jsonl");
        write(&root, "facets/work/entities/alice/not-observations.jsonl");
        write(&root, "facets/work/entities/alice/extra/observations.jsonl");
        write(&root, "chronicle/20260430/default/090000_300/screen.jsonl");
        write(
            &root,
            "chronicle/20260430/default/090000_300/left_screen.jsonl",
        );
        write(
            &root,
            "chronicle/20260430/default/090000_300/talents/documents.json",
        );
        write(
            &root,
            "chronicle/20260430/default/090000_300/talents/speaker_labels.json",
        );

        let files = discover_edge_files(&root).expect("discover edge files");
        let rels: Vec<_> = files.keys().cloned().collect();
        assert_eq!(
            rels,
            vec![
                "20260430/default/090000_300/left_screen.jsonl",
                "20260430/default/090000_300/screen.jsonl",
                "20260430/default/090000_300/talents/documents.json",
                "20260430/default/090000_300/talents/speaker_labels.json",
                "facets/work/activities/20260304.jsonl",
                "facets/work/entities/20260304.jsonl",
                "facets/work/entities/alice/observations.jsonl",
                "facets/work/events/20260304.jsonl",
            ]
        );
        fs::remove_dir_all(root).expect("cleanup edge discovery root");
    }
}
