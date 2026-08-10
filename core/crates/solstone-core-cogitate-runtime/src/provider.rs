// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use solstone_core_generate_wire::{
    ConverseFailure, ConverseMessage, ConverseToolSpec, ConverseTurn,
};

/// A shared wire turn plus the provider response identifier used exclusively
/// for Python-compatible turn-ladder deduplication.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderResponse {
    pub turn: ConverseTurn,
    pub response_id: String,
}

/// Provider boundary for the synchronous, in-process R5a loop.
///
/// The deadline is cooperative: unlike Python's cancellable asyncio task, this
/// wave cannot interrupt a blocking provider call from another thread.
pub trait ConverseProvider {
    fn converse(
        &mut self,
        model: &str,
        system_instruction: Option<&str>,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        deadline: Duration,
    ) -> Result<ProviderResponse, ConverseFailure>;
}
