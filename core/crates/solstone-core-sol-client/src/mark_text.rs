// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Formatting helpers for journal marks using external `spl_core` mark types.
//! Punctuation here must match `solstone_core_sol_link::mark` (`spoken_mark` and `mark_words`).

use spl_core::mark::{MarkError, MarkRenderSpec, mark_from_jid};

pub(crate) fn spoken_mark(spec: &MarkRenderSpec) -> String {
    format!(
        "{}, {} · {}·{}",
        spec.icon1.color.name, spec.icon2.color.name, spec.words[0], spec.words[1]
    )
}

pub(crate) fn mark_words_from_jid(jid: &str) -> Result<String, MarkError> {
    let mark = mark_from_jid(jid)?;
    let spec = mark.to_render_spec();
    Ok(format!("{}·{}", spec.words[0], spec.words[1]))
}
