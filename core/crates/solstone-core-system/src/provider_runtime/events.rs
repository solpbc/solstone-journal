// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Observable provider-runtime events. Transport remains caller-owned.

use super::model::ProviderName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRuntimeEvent {
    Step(&'static str),
    Dispatched {
        operation: &'static str,
        provider: ProviderName,
    },
    StaleResultDiscarded {
        operation: &'static str,
        provider: ProviderName,
    },
    RetryScheduled {
        provider: ProviderName,
    },
    RetryExhausted {
        provider: ProviderName,
    },
    StopDeferred {
        provider: ProviderName,
    },
    CleanupRetry {
        provider: ProviderName,
    },
    RecycleRequested {
        provider: ProviderName,
    },
    GateReleased,
}

pub trait ProviderRuntimeEventSink {
    fn emit(&mut self, event: ProviderRuntimeEvent);
}

#[derive(Debug, Default)]
pub struct VecEventSink {
    pub events: Vec<ProviderRuntimeEvent>,
}

impl ProviderRuntimeEventSink for VecEventSink {
    fn emit(&mut self, event: ProviderRuntimeEvent) {
        self.events.push(event);
    }
}
