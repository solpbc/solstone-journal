// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Possible names one agent connection may see.
//!
//! A connection sees a possible name only while `solstone-core-possible-names`
//! says it still holds, only when its grant includes the entities category (a
//! possible name names an entity), and only for an entity attached to a facet
//! the connection may reach (resolved with [`available_facets`], the same
//! resolution `list_facets` and `list_entities` use). Without the entities
//! category it gets no label and no search hit, and refused rows never count
//! toward a total or a cursor. One instance serves one
//! response: every entry in it is checked against one view of the registry.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use solstone_core_indexer_query::{AdmittedCategory, PossibleNameAdmission};
use solstone_core_possible_names::{
    LiveEntry, LiveSegments, PossibleNameLabels, Viewer, possible_name_label, sidecar_path,
    transcript_labels,
};

use crate::dispatch::available_facets;
use crate::permissions::ConnectionReadSnapshot;

pub(crate) struct ConnectionPossibleNames {
    journal_root: PathBuf,
    snapshot: ConnectionReadSnapshot,
    /// Built on first need: most reads touch no segment with possible names.
    live: OnceLock<LiveSegments>,
    released: Mutex<HashMap<String, bool>>,
}

impl ConnectionPossibleNames {
    pub(crate) fn new(journal_root: &Path, snapshot: &ConnectionReadSnapshot) -> Arc<Self> {
        Arc::new(Self {
            journal_root: journal_root.to_path_buf(),
            snapshot: snapshot.clone(),
            live: OnceLock::new(),
            released: Mutex::new(HashMap::new()),
        })
    }

    /// Entries are checked for the facet directories this connection may
    /// reach; none when they can't be resolved.
    fn live(&self) -> &LiveSegments {
        self.live.get_or_init(|| {
            let facets = available_facets(&self.journal_root, &self.snapshot)
                .map(|facets| facets.into_iter().map(|(_, dir, _)| dir).collect())
                .unwrap_or_else(|_| BTreeSet::new());
            LiveSegments::new(
                &self.journal_root,
                Viewer::Agent {
                    facets,
                    entities: self.entities(),
                },
            )
        })
    }

    /// Whether this connection's grant includes the entities category.
    fn entities(&self) -> bool {
        self.snapshot
            .categories
            .contains(&AdmittedCategory::Entities)
    }

    /// Labels a transcript read shows this connection, bound into its version.
    pub(crate) fn labels(&self, segment_dir: &Path) -> Option<PossibleNameLabels> {
        if !self.entities() || !sidecar_path(segment_dir).exists() {
            return None;
        }
        transcript_labels(&self.live().entries(segment_dir))
    }

    /// The entry a stored search row stands for, if this connection may see
    /// it now and its segment is still live.
    pub(crate) fn indexed(&self, rel: &str, idx: i64, content: &str) -> Option<LiveEntry> {
        if !self.entities() {
            return None;
        }
        let released = *self
            .released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(rel.to_owned())
            .or_insert_with(|| {
                crate::source_release::check_indexed_source_release(&self.journal_root, rel)
            });
        if !released {
            return None;
        }
        self.live().indexed(rel, idx, content)
    }

    /// Search admission for this connection's possible-name rows.
    pub(crate) fn admission(self: &Arc<Self>) -> PossibleNameAdmission {
        let names = Arc::clone(self);
        Arc::new(move |path: &str, idx: i64, content: &str| {
            names.indexed(path, idx, content).is_some()
        })
    }
}

/// What a possible-name search result says: the label, and the words it
/// labels. Only served to a connection that may read the segment's transcript.
pub(crate) fn matched_text(entry: &LiveEntry) -> String {
    format!(
        "{} (heard as \"{}\")",
        possible_name_label(&entry.name),
        entry.heard
    )
}
