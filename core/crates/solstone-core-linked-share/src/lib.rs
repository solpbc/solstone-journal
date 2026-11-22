// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Linked-journal activity sharing groundwork: which activities were time
//! with one linked person, and a summary of each in-scope segment kept to
//! that interaction.
//!
//! Matching is code wherever code is reliable — a live message exchange, a
//! focused 1:1 read, a sustained voice label — and one narrow model question
//! for a spoken name. Summaries cite source lines; code, never the model,
//! writes who did each point. Everything here only reads the journal.

mod cli;
mod lines;
mod live;
mod matching;
mod messages;
mod names;
mod people;
mod source;
mod summary;
mod talents;
mod transport;
pub use transport::transport_diagnostic;

#[cfg(test)]
mod tests;

pub use cli::{
    CliRun, MATCH_USAGE, PACKAGE_USAGE, SHARE_USAGE, reads_journal, run_cli, run_cli_for_locale,
};
pub use live::ActivityWindow;
pub use matching::{ActivityMatch, Basis, Shape, match_activity};
pub use names::{PersonRef, normalize};
pub use people::{Owner, resolve_person};
pub use summary::{SegmentSummary, SharePackage, SharedPoint, build_package};
pub use talents::{ACTIVITY_TALENT, PRESENCE_TALENT, RuntimeTalents, SEGMENT_TALENT, TalentCaller};
