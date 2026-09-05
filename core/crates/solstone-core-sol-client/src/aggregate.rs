// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::command::{CommandContext, CommandOutput};
use crate::generated::inventory;
use crate::resident::ResidentHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryEntry {
    pub surface: &'static str,
    pub path: &'static [&'static str],
    pub kind: &'static str,
    pub help: &'static str,
    pub authority_path: &'static str,
    pub params_json: &'static str,
    pub entry_type: &'static str,
    pub operation_id: &'static str,
    pub method: Option<&'static str>,
    pub route: Option<&'static str>,
    pub contract_operation_id: Option<&'static str>,
    pub handler: &'static str,
    pub resident: bool,
}

pub type Handler = for<'a> fn(CommandContext<'a>) -> CommandOutput;

#[must_use]
pub fn entries() -> &'static [InventoryEntry] {
    inventory::ENTRIES
}

#[must_use]
pub fn handler_bindings() -> &'static [Handler] {
    inventory::HANDLERS
}

#[must_use]
pub fn resident_handler_bindings() -> &'static [ResidentHandler] {
    inventory::RESIDENT_HANDLERS
}

#[must_use]
pub fn handler_for(path: &[&str]) -> Option<(&'static InventoryEntry, Handler)> {
    handler_for_surface("sol-call", path)
}

#[must_use]
pub fn handler_for_surface(
    surface: &str,
    path: &[&str],
) -> Option<(&'static InventoryEntry, Handler)> {
    let mut handler_index = 0;
    for entry in inventory::ENTRIES {
        if entry.resident {
            continue;
        }
        let handler = inventory::HANDLERS
            .get(handler_index)
            .copied()
            .expect("generated buffered handler table must match inventory");
        handler_index += 1;
        if entry.surface == surface && entry.path == path {
            return Some((entry, handler));
        }
    }
    None
}

#[must_use]
pub fn resident_handler_for(path: &[&str]) -> Option<(&'static InventoryEntry, ResidentHandler)> {
    resident_handler_for_surface("sol-call", path)
}

#[must_use]
pub fn resident_handler_for_surface(
    surface: &str,
    path: &[&str],
) -> Option<(&'static InventoryEntry, ResidentHandler)> {
    let mut handler_index = 0;
    for entry in inventory::ENTRIES {
        if !entry.resident {
            continue;
        }
        let handler = inventory::RESIDENT_HANDLERS
            .get(handler_index)
            .copied()
            .expect("generated resident handler table must match inventory");
        handler_index += 1;
        if entry.surface == surface && entry.path == path {
            return Some((entry, handler));
        }
    }
    None
}
