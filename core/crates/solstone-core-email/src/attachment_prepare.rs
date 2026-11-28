// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The sync engine asks the host to read admitted bytes. Publication belongs to sync.
pub use solstone_core_format::attachment::{
    PrepareFailure, PreparedAttachment, SharedAttachmentReader,
};
pub type AttachmentPreparer<'a> =
    dyn Fn(&str, &str, &[u8]) -> Result<PreparedAttachment, PrepareFailure> + 'a;
