// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Formatting helpers for journal marks using external `spl_core` mark types.
//! Punctuation here must match `solstone_core_sol_link::mark` (`spoken_mark` and `mark_words`).

use spl_core::mark::{MarkError, MarkRenderSpec};

/// Said wherever the journal's mark exists but can't be read or worked out.
pub(crate) const MARK_UNAVAILABLE_LINE: &str = "your journal's mark: unavailable right now";
pub(crate) const NO_IDENTITY_YET_LINE: &str =
    "your journal's identity is created when its mark is locked in during setup.";

#[cfg(not(test))]
pub(crate) use spl_core::mark::mark_from_jid;

/// Test builds memoize successful derivations: each one is a deliberately
/// slow Argon2id pass, and the tests reuse a handful of journal IDs.
#[cfg(test)]
pub(crate) fn mark_from_jid(jid: &str) -> Result<spl_core::mark::Mark, MarkError> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    static MARKS: LazyLock<Mutex<HashMap<String, spl_core::mark::Mark>>> =
        LazyLock::new(Default::default);
    if let Some(mark) = MARKS.lock().expect("mark memo").get(jid) {
        return Ok(mark.clone());
    }
    let mark = spl_core::mark::mark_from_jid(jid)?;
    MARKS
        .lock()
        .expect("mark memo")
        .insert(jid.to_owned(), mark.clone());
    Ok(mark)
}

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
