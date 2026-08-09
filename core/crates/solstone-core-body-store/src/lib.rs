// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure in-memory replay of validated body-dedupe observations.

mod error;
mod legacy;
mod replay;
mod row;
mod state;
mod text;

pub use error::{BodyDedupeError, BodyDedupeErrorField};
pub use legacy::{
    LegacyBodyRowError, LegacyBodyRowErrorField, LegacyBodyRowErrorKind, ValidatedLegacyBodyRow,
    validate_legacy_body_row,
};
pub use replay::{
    BodyBundleReplay, BodyBundleReplayError, BodyBundleReplayErrorKind, ValidatedBodyBundleReplay,
};
pub use row::BodyDedupeRow;
pub use state::{BodyDedupeDisposition, BodyDedupeState};
