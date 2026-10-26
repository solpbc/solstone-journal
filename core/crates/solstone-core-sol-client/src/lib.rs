// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod agent_output;
#[cfg(test)]
mod agent_output_tests;
pub mod aggregate;
mod args;
pub mod command;
pub mod decode;
pub mod error;
pub mod generated;
pub mod json_format;
pub mod link_credentials;
mod mark_text;
pub mod pagination;
pub mod resident;
pub mod seam;
pub mod sse;
pub mod transport;
