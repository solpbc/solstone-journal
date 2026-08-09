// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure in-memory replay of validated body-dedupe observations.

mod error;
mod row;
mod state;
mod text;

pub use error::{BodyDedupeError, BodyDedupeErrorField};
pub use row::BodyDedupeRow;
pub use state::{BodyDedupeDisposition, BodyDedupeState};
