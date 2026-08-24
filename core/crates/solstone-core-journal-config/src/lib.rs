// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict journal configuration reads and default materialization.

mod defaults;
mod direct_port;
mod name;
mod notification_labels;
pub mod parakeet_coreml;
mod path;
mod read;

#[cfg(test)]
mod test_support;

pub use defaults::{materialized_defaults, plain_defaults};
pub use direct_port::{
    DEFAULT_DIRECT_DOOR_PORT, DirectDoorPortError, DirectDoorPortValueError,
    direct_door_port_from_config, read_direct_door_port,
};
pub use name::is_path_shaped_name;
pub use notification_labels::{
    SYSTEM_NOTIFICATIONS, SYSTEM_NOTIFICATIONS_LINUX, SYSTEM_NOTIFICATIONS_MACOS,
};
pub use path::get_journal_config_path;
pub use read::{
    ConfigLoadError, JournalConfigMutationBase, JournalConfigRead, load_mutation_base,
    read_journal_config,
};

#[cfg(test)]
mod tests;
