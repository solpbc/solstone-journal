// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Injectable worker and persistence seams for the pure runtime state machine.

use std::collections::BTreeMap;

use super::model::{
    ProviderFence, ProviderName, ProviderRetryState, ProviderRuntimeState, ReasonCode, RuntimePhase,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryToken {
    pub revision: u64,
    pub token_id: String,
    pub desired_fingerprint: Option<String>,
    pub reason_code: ReasonCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStoreError {
    Corrupt,
    Unavailable,
    Conflict,
}

pub fn store_error_phase(error: RuntimeStoreError) -> RuntimePhase {
    match error {
        RuntimeStoreError::Corrupt => RuntimePhase::StateCorrupt,
        RuntimeStoreError::Unavailable | RuntimeStoreError::Conflict => {
            RuntimePhase::StateUnavailable
        }
    }
}

pub trait TruthObservationSeam {
    fn dispatch_truth(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence);
}

/// Start and stop belong together because both own a managed process handle.
pub trait LifecycleSeam {
    fn dispatch_start(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence);
    fn dispatch_stop(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence);
}

pub trait ProbeSeam {
    fn dispatch_probe(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence);
}

pub trait RuntimeStore {
    fn read_retry_token(
        &mut self,
        provider: ProviderName,
    ) -> Result<Option<RetryToken>, RuntimeStoreError>;
    fn consume_retry_token(
        &mut self,
        provider: ProviderName,
        token_id: &str,
    ) -> Result<(), RuntimeStoreError>;
    fn publish_state(&mut self, state: &ProviderRuntimeState) -> Result<(), RuntimeStoreError>;
}

#[derive(Debug, Default)]
pub struct InMemoryRuntimeStore {
    pub retry_tokens: BTreeMap<ProviderName, RetryToken>,
    pub published: Vec<(ProviderName, super::model::RuntimePhase)>,
    pub failure: Option<RuntimeStoreError>,
}

impl RuntimeStore for InMemoryRuntimeStore {
    fn read_retry_token(
        &mut self,
        provider: ProviderName,
    ) -> Result<Option<RetryToken>, RuntimeStoreError> {
        if let Some(error) = self.failure.clone() {
            return Err(error);
        }
        Ok(self.retry_tokens.get(&provider).cloned())
    }

    fn consume_retry_token(
        &mut self,
        provider: ProviderName,
        token_id: &str,
    ) -> Result<(), RuntimeStoreError> {
        if let Some(error) = self.failure.clone() {
            return Err(error);
        }
        match self.retry_tokens.get(&provider) {
            Some(token) if token.token_id == token_id => {
                self.retry_tokens.remove(&provider);
                Ok(())
            }
            _ => Err(RuntimeStoreError::Conflict),
        }
    }

    fn publish_state(&mut self, state: &ProviderRuntimeState) -> Result<(), RuntimeStoreError> {
        if let Some(error) = self.failure.clone() {
            return Err(error);
        }
        self.published.push((state.provider, state.latest_phase));
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct NoopWorkers;

impl TruthObservationSeam for NoopWorkers {
    fn dispatch_truth(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {}
}
impl LifecycleSeam for NoopWorkers {
    fn dispatch_start(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {}
    fn dispatch_stop(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {}
}
impl ProbeSeam for NoopWorkers {
    fn dispatch_probe(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {}
}

pub fn reset_retry(state: &mut ProviderRuntimeState) {
    state.retry = ProviderRetryState {
        desired_fingerprint: state.desired_fingerprint.clone(),
        ..ProviderRetryState::default()
    };
}
