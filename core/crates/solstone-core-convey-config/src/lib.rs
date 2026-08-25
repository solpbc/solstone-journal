// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded ownership of journal `config/convey.json` mutations.

pub mod navigation;

pub use navigation::{
    ConveyConfigError, ConveyConfigMigrationReport, drop_services_navigation,
    pin_curation_navigation, seed_default_app_navigation,
};
