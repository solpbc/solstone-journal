// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict journal configuration reads and default materialization.

mod defaults;
mod path;
mod read;

#[cfg(test)]
mod test_support;

pub use defaults::{materialized_defaults, plain_defaults};
pub use path::get_journal_config_path;
pub use read::{
    ConfigLoadError, JournalConfigMutationBase, JournalConfigRead, load_mutation_base,
    read_journal_config,
};

#[cfg(test)]
mod tests;
