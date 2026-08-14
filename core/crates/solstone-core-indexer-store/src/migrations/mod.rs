// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-time index-store migrations.
//!
//! These run once against a journal whose index predates a layout the current
//! schema assumes. They are not part of ordinary indexing and are never invoked
//! by a scan.

pub mod index_stream;
