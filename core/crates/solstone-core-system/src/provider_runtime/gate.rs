// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-provider startup gate state, without an external readiness consumer.

use std::collections::{BTreeMap, BTreeSet};

use super::events::{ProviderRuntimeEvent, ProviderRuntimeEventSink};
use super::model::{
    LaunchOutcomeStatus, PROVIDER_STARTUP_GATE_CEILING_SECONDS,
    PROVIDER_STARTUP_GATE_WINDOW_SECONDS, PROVIDER_STARTUP_TERMINAL_PHASES, ProviderName,
    ProviderRuntimeNow, RuntimePhase, phase_in,
};

#[derive(Debug, Clone)]
pub struct ProviderStartupGate {
    pub started_at: f64,
    pub required: BTreeSet<ProviderName>,
    pub terminal: BTreeSet<ProviderName>,
    pub attempted: BTreeMap<ProviderName, LaunchOutcomeStatus>,
    pub first_start_at: Option<f64>,
    pub released: bool,
}

impl ProviderStartupGate {
    pub fn new(now: ProviderRuntimeNow, required: impl IntoIterator<Item = ProviderName>) -> Self {
        Self {
            started_at: now.monotonic_seconds,
            required: required.into_iter().collect(),
            terminal: BTreeSet::new(),
            attempted: BTreeMap::new(),
            first_start_at: None,
            released: false,
        }
    }

    pub fn on_start_submitted(&mut self, provider: ProviderName, now: ProviderRuntimeNow) {
        if self.required.contains(&provider) && self.first_start_at.is_none() {
            self.first_start_at = Some(now.monotonic_seconds);
        }
    }

    pub fn on_start_result(&mut self, provider: ProviderName, status: LaunchOutcomeStatus) {
        if self.required.contains(&provider) {
            self.attempted.entry(provider).or_insert(status);
        }
    }

    pub fn on_phase(&mut self, provider: ProviderName, phase: RuntimePhase) {
        if self.required.contains(&provider) && phase_in(&PROVIDER_STARTUP_TERMINAL_PHASES, phase) {
            self.terminal.insert(provider);
        }
    }

    pub fn release_if_ready(
        &mut self,
        now: ProviderRuntimeNow,
        sink: &mut dyn ProviderRuntimeEventSink,
    ) -> bool {
        if self.released {
            return false;
        }
        let mut satisfied = self.terminal.clone();
        satisfied.extend(self.attempted.keys().copied());
        let first_start_expired = self.first_start_at.is_some_and(|start| {
            now.monotonic_seconds - start >= PROVIDER_STARTUP_GATE_CEILING_SECONDS
        });
        let window_expired =
            now.monotonic_seconds - self.started_at >= PROVIDER_STARTUP_GATE_WINDOW_SECONDS;
        if self.required.is_subset(&satisfied) || first_start_expired || window_expired {
            self.released = true;
            sink.emit(ProviderRuntimeEvent::GateReleased);
            return true;
        }
        false
    }
}
