// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Mutex;

use crate::cadence::AttestationSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestationFailureKind {
    Failed,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationFailure {
    pub kind: AttestationFailureKind,
    pub reason_code: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttestationState {
    pub session: Option<AttestationSession>,
    pub failure: Option<AttestationFailure>,
    pub last_verified: Option<AttestationSession>,
}

#[derive(Default)]
pub struct AttestationStateStore {
    state: Mutex<AttestationState>,
}

impl AttestationStateStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get_attestation_state(&self) -> AttestationState {
        self.state
            .lock()
            .expect("attestation state lock poisoned")
            .clone()
    }
    pub fn record_attestation_verified(&self, session: AttestationSession) {
        *self.state.lock().expect("attestation state lock poisoned") = AttestationState {
            session: Some(session.clone()),
            failure: None,
            last_verified: Some(session),
        };
    }
    pub fn record_attestation_failed(
        &self,
        kind: AttestationFailureKind,
        reason_code: &'static str,
    ) {
        let mut state = self.state.lock().expect("attestation state lock poisoned");
        state.session = None;
        state.failure = Some(AttestationFailure { kind, reason_code });
    }
    pub fn clear_attestation_state(&self) {
        let mut state = self.state.lock().expect("attestation state lock poisoned");
        state.session = None;
        state.failure = None;
    }
    pub fn delete_attestation_state(&self) {
        *self.state.lock().expect("attestation state lock poisoned") = AttestationState::default();
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use solstone_core_spp_attest::{
        nvgpu::claims::GpuAppraisal,
        snp::{CpuAppraisal, CpuTcb, TcbVersion},
    };

    use super::*;
    use crate::cadence::CompositeVerdict;

    fn session() -> AttestationSession {
        let tcb = TcbVersion {
            boot_loader: None,
            tee: None,
            snp: None,
            microcode: None,
            fmc: None,
        };
        AttestationSession {
            verdict: CompositeVerdict {
                cpu: CpuAppraisal {
                    steps: Vec::new(),
                    hcla_version: 0,
                    report_version: 0,
                    cpuid_family: None,
                    cpuid_model: None,
                    cpuid_step: None,
                    tcb: CpuTcb {
                        current: tcb.clone(),
                        reported: tcb.clone(),
                        committed: tcb.clone(),
                        launch: tcb,
                    },
                    pcr_sha256: String::new(),
                    host_data_hex: String::new(),
                    measurement_hex: String::new(),
                    chip_id_hex: String::new(),
                },
                gpu: GpuAppraisal {
                    steps: Vec::new(),
                    driver_version: String::new(),
                    vbios_version: String::new(),
                    hwmodel: String::new(),
                    ueid: String::new(),
                    oemid: String::new(),
                    eat_nonce: String::new(),
                    claims_version: String::new(),
                    arch: String::new(),
                    envelope_gpu_uuid: String::new(),
                },
            },
            started_at: UNIX_EPOCH,
            tpm_heartbeat_at: UNIX_EPOCH,
            gpu_reattest_at: UNIX_EPOCH,
        }
    }

    #[test]
    fn transitions_preserve_last_verified_until_delete() {
        let store = AttestationStateStore::new();
        let verified = session();
        store.record_attestation_verified(verified.clone());
        assert_eq!(
            store.get_attestation_state(),
            AttestationState {
                session: Some(verified.clone()),
                failure: None,
                last_verified: Some(verified.clone())
            }
        );
        store.record_attestation_failed(AttestationFailureKind::Failed, "certificate_invalid");
        assert_eq!(
            store.get_attestation_state().last_verified,
            Some(verified.clone())
        );
        assert!(store.get_attestation_state().session.is_none());
        assert_eq!(
            store.get_attestation_state().failure,
            Some(AttestationFailure {
                kind: AttestationFailureKind::Failed,
                reason_code: "certificate_invalid"
            })
        );
        store.record_attestation_failed(AttestationFailureKind::Unreachable, "gateway_unreachable");
        assert_eq!(
            store.get_attestation_state().last_verified,
            Some(verified.clone())
        );
        assert_eq!(
            store.get_attestation_state().failure,
            Some(AttestationFailure {
                kind: AttestationFailureKind::Unreachable,
                reason_code: "gateway_unreachable"
            })
        );
        store.clear_attestation_state();
        assert_eq!(
            store.get_attestation_state(),
            AttestationState {
                session: None,
                failure: None,
                last_verified: Some(verified)
            }
        );
        store.delete_attestation_state();
        assert_eq!(store.get_attestation_state(), AttestationState::default());
    }
}
