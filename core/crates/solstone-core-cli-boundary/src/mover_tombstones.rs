// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Replacement guidance for retired journal mover verbs.

pub const JOURNAL_EXPORT_TOMBSTONE: &str =
    "journal export is retired; use journal transfer send --to LABEL";
pub const TRANSFER_EXPORT_TOMBSTONE: &str =
    "journal transfer export is retired; use journal archive export";
pub const TRANSFER_IMPORT_TOMBSTONE: &str =
    "journal transfer import is retired; use journal archive merge";
