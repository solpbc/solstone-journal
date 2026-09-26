// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Async-to-sync serving bridge for thread-affine entity and facet stores, not a route surface.
//!
//! [`held_delete`] is the durable hold behind the owner's confirmed deletes:
//! the cancel window, the resume at start, and the recorded outcome.

pub mod held_delete;
pub mod seam;
