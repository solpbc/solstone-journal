// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded ownership of journal `config/convey.json` mutations.

pub mod navigation;

pub use navigation::{
    ConveyConfigError, ConveyConfigMigrationReport, ConveyUpdate, clear_facet_references,
    drop_services_navigation, pin_curation_navigation, prepare_remove_facet_references,
    publish_update, rename_facet_references, restore_update, seed_default_app_navigation,
};
