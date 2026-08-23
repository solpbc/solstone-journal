// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only home-dashboard readers and projections.
//!
//! # Declared divergences
//!
//! Day stats use `chronicle/<day>/stats.json`; the no-observer CTA is
//! `/app/network/` and the no-observer glance is `calm`/`neutral` rather than
//! the frozen `ok`/`green`; weekly reflections omit their dead `url`; awareness
//! reads never create `awareness/`; briefing lateness is a function of supplied phase
//! and time; observer timestamps are milliseconds rather than bridge seconds;
//! pipeline `failed` and `outstanding_failed` stay distinct; and calendar math
//! is pinned to UTC.
//!
//! The populated connections projection is source-derived but unoracled pending
//! a captured oracle. `flow_updated_at` is host-dependent (`stat().st_mtime`) and
//! cannot be controlled by clock injection. `ENTITIES_COPY` and
//! `ATTENDANCE_KINDS` currently live in an axum-declaring crate; the long-term
//! shape is to move that data to an axum-free crate.

pub mod briefing;
pub mod connections;
pub mod context;
pub mod formatting;
pub mod health_glance;
pub mod model;
pub mod needs_you;
pub mod pulse;
pub mod readers;

#[cfg(test)]
mod corpus;

pub use context::HomeContext;
