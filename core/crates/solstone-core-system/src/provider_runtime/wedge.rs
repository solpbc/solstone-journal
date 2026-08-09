// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded cortex-event attribution and local-provider recycle decisions.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::events::{ProviderRuntimeEvent, ProviderRuntimeEventSink};
use super::model::{
    LOCAL_WEDGE_PROVIDER_MAP_CAP, LOCAL_WEDGE_RECYCLE_GRACE_SECONDS, LOCAL_WEDGE_THRESHOLD,
    ProviderName, ProviderRuntimeNow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CortexEventKind {
    Start,
    Finish,
    Error,
}

#[derive(Debug, Clone)]
pub struct CortexOutcomeEvent {
    pub kind: CortexEventKind,
    pub use_id: String,
    pub provider: Option<ProviderName>,
    pub reason_code: Option<String>,
}

#[derive(Debug, Default)]
pub struct WedgeState {
    providers: BTreeMap<String, ProviderName>,
    order: VecDeque<String>,
    failures: BTreeSet<String>,
    cooldown_until: f64,
    awaiting_recovery: bool,
}

impl WedgeState {
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }
    pub fn contains_use_id(&self, use_id: &str) -> bool {
        self.providers.contains_key(use_id)
    }
    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    /// Returns a recycle request only for the third attributed local failure outside grace.
    pub fn observe(
        &mut self,
        event: CortexOutcomeEvent,
        now: ProviderRuntimeNow,
    ) -> Option<ProviderName> {
        match event.kind {
            CortexEventKind::Start => {
                if let Some(provider) = event.provider {
                    if !self.providers.contains_key(&event.use_id) {
                        self.order.push_back(event.use_id.clone());
                    }
                    self.providers.insert(event.use_id, provider);
                    while self.providers.len() > LOCAL_WEDGE_PROVIDER_MAP_CAP {
                        if let Some(oldest) = self.order.pop_front() {
                            self.providers.remove(&oldest);
                        }
                    }
                }
                None
            }
            CortexEventKind::Finish => {
                if self.providers.get(&event.use_id) == Some(&ProviderName::Local) {
                    self.failures.clear();
                    self.awaiting_recovery = false;
                }
                None
            }
            CortexEventKind::Error => {
                if self.providers.get(&event.use_id) != Some(&ProviderName::Local)
                    || event.reason_code.as_deref() != Some("provider_unavailable")
                    || now.monotonic_seconds < self.cooldown_until
                {
                    return None;
                }
                self.failures.insert(event.use_id);
                if self.failures.len() < LOCAL_WEDGE_THRESHOLD {
                    return None;
                }
                self.failures.clear();
                self.awaiting_recovery = true;
                self.cooldown_until = now.monotonic_seconds + LOCAL_WEDGE_RECYCLE_GRACE_SECONDS;
                Some(ProviderName::Local)
            }
        }
    }
}

/// Routes one cortex outcome through bounded wedge tracking and emits an eligible recycle request.
pub fn observe_cortex_outcome(
    wedge: &mut WedgeState,
    event: CortexOutcomeEvent,
    now: ProviderRuntimeNow,
    sink: &mut dyn ProviderRuntimeEventSink,
) -> Option<ProviderName> {
    let provider = wedge.observe(event, now)?;
    sink.emit(ProviderRuntimeEvent::RecycleRequested { provider });
    Some(provider)
}
