// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::ParsedMessage;
use crate::model::ElementKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CoverageCell {
    pub bundle: String,
    pub locale: String,
    pub message_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CoverageReport {
    pub bundle_names: Vec<String>,
    pub locale_tags: Vec<String>,
    pub message_ids: Vec<String>,
    pub cells: Vec<CoverageCell>,
    pub messages_num: usize,
    pub messages_den: usize,
    pub branches_num: usize,
    pub branches_den: usize,
    pub slots_num: usize,
    pub slots_den: usize,
}

impl CoverageReport {
    pub fn compute(
        bundle_locales: &HashMap<String, Vec<String>>,
        message_bundles: &HashMap<String, Vec<String>>,
        source_messages: &HashMap<String, ParsedMessage>,
        translations: &HashMap<String, HashMap<String, ParsedMessage>>,
        stale_message_ids: &HashSet<String>,
    ) -> Self {
        let mut cells_set = BTreeSet::new();

        for (msg_id, bundles) in message_bundles {
            if !source_messages.contains_key(msg_id) {
                continue;
            }
            for bundle in bundles {
                if let Some(locales) = bundle_locales.get(bundle) {
                    for loc in locales {
                        cells_set.insert(CoverageCell {
                            bundle: bundle.clone(),
                            locale: loc.clone(),
                            message_id: msg_id.clone(),
                        });
                    }
                }
            }
        }

        let cells: Vec<CoverageCell> = cells_set.into_iter().collect();

        let mut bundle_names_set = BTreeSet::new();
        let mut locale_tags_set = BTreeSet::new();
        let mut message_ids_set = BTreeSet::new();

        for cell in &cells {
            bundle_names_set.insert(cell.bundle.clone());
            locale_tags_set.insert(cell.locale.clone());
            message_ids_set.insert(cell.message_id.clone());
        }

        let bundle_names: Vec<String> = bundle_names_set.into_iter().collect();
        let locale_tags: Vec<String> = locale_tags_set.into_iter().collect();
        let message_ids: Vec<String> = message_ids_set.into_iter().collect();

        let messages_den = message_ids.len();
        let mut branches_den = 0;
        let mut slots_den = 0;

        for cell in &cells {
            if let Some(src) = source_messages.get(&cell.message_id) {
                if src.selector_var.is_some() {
                    branches_den += src.branches.len();
                }
                for b in &src.branches {
                    let slot_count = b
                        .pattern
                        .iter()
                        .filter(|el| el.kind == ElementKind::Slot)
                        .count();
                    slots_den += slot_count;
                }
            }
        }

        let mut branches_num = 0;
        let mut slots_num = 0;

        for cell in &cells {
            if stale_message_ids.contains(&cell.message_id) {
                continue;
            }
            let src = match source_messages.get(&cell.message_id) {
                Some(s) => s,
                None => continue,
            };
            let tr_msg = match translations
                .get(&cell.locale)
                .and_then(|m| m.get(&cell.message_id))
            {
                Some(m) => m,
                None => continue,
            };

            // Branches numerator: count source branch keys present in translation
            if src.selector_var.is_some() {
                let tr_keys: HashSet<Option<&str>> =
                    tr_msg.branches.iter().map(|b| b.key.as_deref()).collect();
                for src_b in &src.branches {
                    if tr_keys.contains(&src_b.key.as_deref()) {
                        branches_num += 1;
                    }
                }
            }

            // Slots numerator: count slots matching source slots
            if src.selector_var.is_none() {
                let src_slots = src
                    .branches
                    .first()
                    .map(|b| {
                        b.pattern
                            .iter()
                            .filter(|el| el.kind == ElementKind::Slot)
                            .count()
                    })
                    .unwrap_or(0);
                let tr_slots = tr_msg
                    .branches
                    .first()
                    .map(|b| {
                        b.pattern
                            .iter()
                            .filter(|el| el.kind == ElementKind::Slot)
                            .count()
                    })
                    .unwrap_or(0);
                slots_num += tr_slots.min(src_slots);
            } else {
                for src_b in &src.branches {
                    let src_slots = src_b
                        .pattern
                        .iter()
                        .filter(|el| el.kind == ElementKind::Slot)
                        .count();
                    if let Some(tr_b) = tr_msg.branches.iter().find(|b| b.key == src_b.key) {
                        let tr_slots = tr_b
                            .pattern
                            .iter()
                            .filter(|el| el.kind == ElementKind::Slot)
                            .count();
                        slots_num += tr_slots.min(src_slots);
                    }
                }
            }
        }

        // Messages numerator: distinct message IDs that have all their cells covered by non-stale translations
        let mut messages_num = 0;
        for msg_id in &message_ids {
            if stale_message_ids.contains(msg_id) {
                continue;
            }
            let msg_cells: Vec<&CoverageCell> =
                cells.iter().filter(|c| &c.message_id == msg_id).collect();
            let all_covered = !msg_cells.is_empty()
                && msg_cells.iter().all(|c| {
                    translations
                        .get(&c.locale)
                        .and_then(|m| m.get(msg_id))
                        .is_some()
                });
            if all_covered {
                messages_num += 1;
            }
        }

        Self {
            bundle_names,
            locale_tags,
            message_ids,
            cells,
            messages_num,
            messages_den,
            branches_num,
            branches_den,
            slots_num,
            slots_den,
        }
    }

    pub fn format_stdout(&self) -> String {
        format!(
            "Coverage Report:\n\
             Bundles: {}\n\
             Locales: {}\n\
             Population: {} messages\n\
             Messages: {} / {}\n\
             Selector branches: {} / {}\n\
             Protected slots: {} / {}\n",
            self.bundle_names.join(", "),
            self.locale_tags.join(", "),
            self.message_ids.len(),
            self.messages_num,
            self.messages_den,
            self.branches_num,
            self.branches_den,
            self.slots_num,
            self.slots_den
        )
    }
}
