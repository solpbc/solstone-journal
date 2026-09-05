// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_MAX_TURNS: usize = 60;
const DEFAULT_READ_CALL_BUDGET: i64 = 200;

/// Effective configuration for one bounded cogitate run.
///
/// Prompt composition is performed by the native cogitate-wire request boundary
/// before this runtime receives `RunInput`. The runtime consumes the composed
/// instruction and an explicit journal root rather than looking up either from
/// the environment.
#[derive(Clone, Debug, PartialEq)]
pub struct RunConfig {
    pub access_tier: String,
    pub outbound_approval: Option<String>,
    pub expects_emit_final: bool,
    pub max_turns: usize,
    pub context_window: Option<u64>,
    pub timeout: Duration,
    /// Used by the caller when constructing its `ToolExecutor` (for example,
    /// `CogitateToolExecutor::new`); `run_cogitate` does not consume it.
    pub read_call_budget: i64,
    pub model: String,
    pub correlation_id: String,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            access_tier: "normal".to_owned(),
            outbound_approval: None,
            expects_emit_final: false,
            max_turns: DEFAULT_MAX_TURNS,
            context_window: None,
            timeout: Duration::from_secs(600),
            read_call_budget: DEFAULT_READ_CALL_BUDGET,
            model: String::new(),
            correlation_id: String::new(),
        }
    }
}

impl RunConfig {
    /// Match Python's in-process deadline: leave 30 seconds for teardown, or
    /// use half a too-small timeout so the deadline remains strictly inside it.
    /// This permits a clean outcome before Cortex's SIGTERM timer fires rather
    /// than killing the run mid-flight without an outcome.
    pub fn wall_clock_deadline(&self) -> Duration {
        const GRACE: Duration = Duration::from_secs(30);
        self.timeout
            .checked_sub(GRACE)
            .filter(|deadline| !deadline.is_zero())
            .unwrap_or(self.timeout / 2)
    }
}

/// Launch-only values which are not part of a talent's effective policy.
#[derive(Clone, Debug, PartialEq)]
pub struct RunInput {
    pub config: RunConfig,
    pub initial_prompt: String,
    pub system_instruction: Option<String>,
    /// Used by the caller when constructing its `ToolExecutor`; pluggable
    /// executors need not use it and `run_cogitate` does not consume it.
    pub journal_root: PathBuf,
}
