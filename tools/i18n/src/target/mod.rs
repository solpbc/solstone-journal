// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Not implemented targets: InfoPlist, App Shortcuts, .desktop, AppStream, winget, store listings.

pub mod android;
pub mod apple;
pub mod cases;
pub mod esm;
pub mod ftl;
pub mod gettext;
pub mod model;
pub mod oracle;
pub mod publish;
pub mod readers;
pub mod rust;
pub mod webextension;

#[cfg(test)]
pub mod tests;

pub use model::Target;
pub use publish::publish;
