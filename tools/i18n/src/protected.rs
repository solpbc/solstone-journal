// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::model::{ElementKind, PatternElement};
use std::collections::HashMap;

pub fn verify_branch_protected_slots(
    msg_id: &str,
    locale: &str,
    source_pattern: &[PatternElement],
    translation_pattern: &[PatternElement],
) -> Result<(), Fault> {
    let mut src_counts: HashMap<&str, usize> = HashMap::new();
    for el in source_pattern {
        if el.kind == ElementKind::Slot {
            *src_counts.entry(&el.value).or_insert(0) += 1;
        }
    }

    let mut tr_counts: HashMap<&str, usize> = HashMap::new();
    for el in translation_pattern {
        if el.kind == ElementKind::Slot {
            *tr_counts.entry(&el.value).or_insert(0) += 1;
        }
    }

    if src_counts != tr_counts {
        return Err(Fault::new("protected-slot-mismatch", locale, msg_id));
    }

    Ok(())
}
