// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::{Duration, SystemTime};

use solstone_core_spp_attest::{nvgpu::claims::GpuAppraisal, snp::CpuAppraisal};

pub const TPM_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10 * 60);
pub const GPU_REATTEST_INTERVAL: Duration = Duration::from_secs(30 * 60);
pub const SESSION_CAP: Duration = Duration::from_secs(60 * 60);

/// Successful composite CPU and GPU attestation verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeVerdict {
    pub verified: bool,
    pub legs: [&'static str; 2],
    pub substrate: String,
    pub checked_at: SystemTime,
    pub cpu: CpuAppraisal,
    pub gpu: GpuAppraisal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationSession {
    pub verdict: CompositeVerdict,
    pub started_at: SystemTime,
    pub tpm_heartbeat_at: SystemTime,
    pub gpu_reattest_at: SystemTime,
}

impl AttestationSession {
    pub fn tpm_heartbeat_due_at(&self) -> SystemTime {
        self.tpm_heartbeat_at + TPM_HEARTBEAT_INTERVAL
    }
    pub fn gpu_reattest_due_at(&self) -> SystemTime {
        self.gpu_reattest_at + GPU_REATTEST_INTERVAL
    }
    pub fn session_cap_at(&self) -> SystemTime {
        self.started_at + SESSION_CAP
    }
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.tpm_heartbeat_due_at()
            || now >= self.gpu_reattest_due_at()
            || now >= self.session_cap_at()
        {
            "stale"
        } else {
            "verified"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use solstone_core_spp_attest::{
        nvgpu::claims::GpuAppraisal,
        snp::{CpuAppraisal, CpuTcb, TcbVersion},
    };

    use super::*;

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
                verified: true,
                legs: ["cpu", "gpu"],
                substrate: String::new(),
                checked_at: UNIX_EPOCH,
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
    fn every_deadline_is_verified_before_and_stale_at_its_boundary() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        let cases = [
            AttestationSession {
                tpm_heartbeat_at: now - Duration::from_secs(10 * 60),
                gpu_reattest_at: now,
                started_at: now,
                ..session()
            },
            AttestationSession {
                tpm_heartbeat_at: now,
                gpu_reattest_at: now - Duration::from_secs(30 * 60),
                started_at: now,
                ..session()
            },
            AttestationSession {
                tpm_heartbeat_at: now,
                gpu_reattest_at: now,
                started_at: now - Duration::from_secs(60 * 60),
                ..session()
            },
        ];
        for item in cases {
            assert_eq!(item.status(now - Duration::from_secs(1)), "verified");
            assert_eq!(item.status(now), "stale");
        }
    }

    #[test]
    fn any_single_expired_deadline_makes_the_session_stale() {
        let base = UNIX_EPOCH + SESSION_CAP;
        let cases = [
            AttestationSession {
                tpm_heartbeat_at: base - Duration::from_secs(10 * 60),
                gpu_reattest_at: base,
                started_at: base,
                ..session()
            },
            AttestationSession {
                tpm_heartbeat_at: base,
                gpu_reattest_at: base - Duration::from_secs(30 * 60),
                started_at: base,
                ..session()
            },
            AttestationSession {
                tpm_heartbeat_at: base,
                gpu_reattest_at: base,
                started_at: base - Duration::from_secs(60 * 60),
                ..session()
            },
        ];
        for item in cases {
            assert_eq!(item.status(base), "stale");
        }
    }
}
