// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a connection may see, decided in one place for each kind of thing.
//!
//! A list calls these to choose its rows. A read calls them again for the one
//! thing it was handed, whoever handed it over and however long ago.
//!
//! ⛔ A check that runs only where a list builds its rows is one a read can be
//! handed its way around: by a reference from before the owner hid something,
//! or by anything an agent can write for itself. Add a rule here, never beside
//! one caller.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_format::content::body_records::BODY_RECORDS_FILE;
use solstone_core_journal_io::paths::Segment;

use crate::body_source::BodyHides;
use solstone_core_indexer_query::ConnectionBoundary;
use solstone_core_indexer_store::classification::{FacetDeclarationSet, classify_source};

use crate::dispatch::{
    available_facets, live_allows, segment_assignment_probe_path, segment_boundary,
};
use crate::permissions::ConnectionReadSnapshot;

/// Whether a connection sees one activity record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Shown {
    Yes,
    /// Not there for this connection: no answer mentions it.
    No,
    /// Left out only for want of the body permission, which an answer may say.
    NeedsBody,
}

/// One facet's activity day file, as every reader of it judges its records.
pub(crate) struct ActivityDay<'a> {
    journal_root: &'a Path,
    snapshot: &'a ConnectionReadSnapshot,
    day: &'a str,
    superseded: BTreeSet<String>,
}

impl<'a> ActivityDay<'a> {
    /// `rows` are the file's records as the strict reader gives them.
    pub(crate) fn new(
        journal_root: &'a Path,
        snapshot: &'a ConnectionReadSnapshot,
        facet_dir: &str,
        day: &'a str,
        rows: &[Map<String, Value>],
    ) -> Self {
        Self {
            journal_root,
            snapshot,
            day,
            superseded: solstone_core_facets::superseded_record_ids(
                journal_root,
                facet_dir,
                day,
                rows,
            ),
        }
    }

    /// The same, for a reader that holds no parsed rows yet. `None` when the
    /// file can't be read strictly, which no list shows either.
    pub(crate) fn read(
        journal_root: &'a Path,
        snapshot: &'a ConnectionReadSnapshot,
        facet_dir: &str,
        day: &'a str,
    ) -> Option<Self> {
        let rows = solstone_core_facets::read_activity_records_strict(journal_root, facet_dir, day)
            .ok()?;
        Some(Self::new(journal_root, snapshot, facet_dir, day, &rows))
    }

    /// Whether this connection sees `record`, a line of this file. A body
    /// record that passes is still shown only as the face its reader composes.
    pub(crate) fn shows(&self, record: &Map<String, Value>) -> Shown {
        // Folded into a code record, or moved to another day or facet.
        if record
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| self.superseded.contains(id))
        {
            return Shown::No;
        }
        if record
            .get("hidden")
            .is_some_and(solstone_core_facets::activity_value_truthy)
            || solstone_core_facets::is_retired_plan(record)
        {
            return Shown::No;
        }
        if crate::body_source::strava_line_hidden(
            self.journal_root,
            self.snapshot,
            self.day,
            record,
        ) {
            return if crate::body_source::strava_line_needs_body(
                self.journal_root,
                self.snapshot,
                self.day,
                record,
            ) {
                Shown::NeedsBody
            } else {
                Shown::No
            };
        }
        if solstone_core_facets::is_body_record(record)
            && !crate::body_source::holds_body(self.snapshot)
        {
            return Shown::NeedsBody;
        }
        Shown::Yes
    }
}

/// Where one facet's activities for a day are kept, for a facet and day this
/// connection's own list could have named.
pub(crate) struct ActivityPlace {
    pub facet_id: String,
    pub facet_dir: String,
    pub day: String,
    pub rel_path: String,
}

/// The place `facet` (its id or its folder) keeps `day`'s activities, when the
/// facet is one this connection reaches and the day is one it has a file for.
///
/// 🔑 The folder and the day in the result come from the journal's own
/// listing, never from the caller's text, so nothing a caller writes is joined
/// into a path. A folder the journal doesn't declare as a facet (merge
/// scratch, a backup) is no place at all.
pub(crate) fn activity_place(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet: &str,
    day: &str,
) -> Option<ActivityPlace> {
    if !solstone_core_journal_io::is_day_key(day) {
        return None;
    }
    let (facet_id, facet_dir) = reached_facet(journal_root, snapshot, facet)?;
    let day = solstone_core_facets::list_activity_day_stems(journal_root, &facet_dir)
        .ok()?
        .into_iter()
        .find(|stem| stem == day)?;
    let rel_path = format!("facets/{facet_dir}/activities/{day}.jsonl");
    Some(ActivityPlace {
        facet_id,
        facet_dir,
        day,
        rel_path,
    })
}

/// The record an activity read was asked for, as this connection sees it.
pub(crate) enum SeenRecord {
    /// A body record: served only as the face its reader composes.
    Body(Map<String, Value>),
    Activity(Map<String, Value>),
}

pub(crate) struct SeenActivity {
    pub place: ActivityPlace,
    /// The day file's digest now, which a paging cursor pins.
    pub file_digest: String,
    pub record: SeenRecord,
}

/// The one record `identity` names in a facet's day, when this connection's
/// own list would show it now: scope first, then the file, then the same
/// test the lists make. `pinned`, from a sealed reference, also refuses a
/// file that changed since it was listed.
pub(crate) fn activity_at(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet: &str,
    day: &str,
    identity: &str,
    pinned: Option<&str>,
) -> Option<SeenActivity> {
    let place = activity_place(journal_root, snapshot, facet, day)?;
    if !live_allows(
        journal_root,
        &crate::dispatch::boundary(snapshot, None, None),
        &place.rel_path,
        None,
    ) {
        return None;
    }
    let bytes = crate::activity_admit::read_contained_source(journal_root, &place.rel_path)?;
    let file_digest = crate::activity_admit::hex_encode(&Sha256::digest(&bytes));
    if pinned.is_some_and(|pinned| pinned != file_digest) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let record = match crate::activity_admit::body_record_by_identity(&text, identity) {
        Some(body) => SeenRecord::Body(body),
        None => SeenRecord::Activity(crate::activity_admit::record_by_identity(&text, identity)?.0),
    };
    let judged = ActivityDay::read(journal_root, snapshot, &place.facet_dir, &place.day)?;
    let (SeenRecord::Body(row) | SeenRecord::Activity(row)) = &record;
    if judged.shows(row) != Shown::Yes {
        return None;
    }
    Some(SeenActivity {
        place,
        file_digest,
        record,
    })
}

/// The facet `facet` names among those this connection reaches: by its id
/// first, so a folder that happens to be named like another facet's id never
/// stands in for it, then by its folder.
pub(crate) fn reached_facet(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    facet: &str,
) -> Option<(String, String)> {
    let facets = available_facets(journal_root, snapshot).ok()?;
    facets
        .iter()
        .find(|(id, _, _)| id == facet)
        .or_else(|| facets.iter().find(|(_, dir, _)| dir == facet))
        .map(|(id, dir, _)| (id.clone(), dir.clone()))
}

/// Whether a segment holding only body records is a window a source's list
/// shows: it holds a records file with something still in it, under a key on
/// the source's grid. A Strava tile
/// and a window whose rows all moved away are read through the activity
/// recorded for them, never as a segment.
pub(crate) fn is_body_window(segment: &Segment) -> bool {
    segment.path().join(BODY_RECORDS_FILE).is_file()
        && !crate::body_source::holds_only_moved(segment.path())
        && segment
            .name()
            .to_str()
            .is_some_and(|name| crate::body_source::grid_clock(name).is_some())
}

/// What of one segment this connection may read: `None` when nothing, else
/// the body rows to leave out of it. `facet` narrows the connection's scope to
/// one facet, as a list asked for one does.
///
/// Scope is decided first, before anything of the segment is opened. The
/// owner's deletion is checked by the caller around its own read of the page.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segment_shown(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    hides: &BodyHides,
    facet: Option<&str>,
    day: &str,
    stream: &str,
    name: &str,
    segment: &Segment,
) -> Option<BTreeSet<String>> {
    let boundary = segment_boundary(snapshot, facet)?;
    let declarations = FacetDeclarationSet::from_journal(journal_root).ok()?;
    segment_shown_within(
        journal_root,
        &boundary,
        &declarations,
        hides,
        day,
        stream,
        name,
        segment,
    )
}

/// Whether a segment is inside this connection's scope and categories, decided
/// from where it is filed alone: nothing of the segment is opened.
pub(crate) fn segment_in_scope(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    day: &str,
    stream: &str,
    name: &str,
) -> bool {
    segment_boundary(snapshot, None).is_some_and(|boundary| {
        live_allows(
            journal_root,
            &boundary,
            &segment_assignment_probe_path(day, stream, name),
            Some(stream),
        )
    })
}

/// [`segment_shown`] for a caller judging many segments in one answer, which
/// reads the connection's boundary and the facet declarations once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segment_shown_within(
    journal_root: &Path,
    boundary: &ConnectionBoundary,
    declarations: &FacetDeclarationSet,
    hides: &BodyHides,
    day: &str,
    stream: &str,
    name: &str,
    segment: &Segment,
) -> Option<BTreeSet<String>> {
    let classification = classify_source(
        journal_root,
        &segment_assignment_probe_path(day, stream, name),
        Some(stream),
        declarations,
    );
    if !boundary.allows_classification(journal_root, &classification) {
        return None;
    }
    // Calendar notes are found through search, never read as a segment.
    if solstone_core_segment::is_calendar_segment(journal_root, segment) {
        return None;
    }
    if solstone_core_segment::is_body_only_segment(segment) && !is_body_window(segment) {
        return None;
    }
    hides.shown(journal_root, day, stream, segment)
}
