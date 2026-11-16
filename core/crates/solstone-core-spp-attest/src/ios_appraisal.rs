// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! iOS out-of-process GPU appraisal state machine, classifier bridge, and
//! host callback transport registry.

use std::sync::Mutex;
use std::time::SystemTime;

use crate::error::GpuAppraisalReason;
use crate::ios_service_tools::{IosVerifierRefusal, admitted_ios_verifier};
use crate::nvgpu::appraise::evidence_json;
use crate::nvgpu::{
    GpuAppraisal, GpuProfile, NvattestAcceptance, NvattestVerdict, StatusExpectation,
    StatusMode,
};
use crate::snp::AppraisalStep;
use crate::tlv::GpuEnvelope;

const MAX_NVATTEST_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_EVIDENCE_JSON_BYTES: usize = 1_048_576;
const MAX_PROOFS_BYTES: usize = 16_384;
const MAX_DETACHED_EAT_ENTRIES: usize = 16;
const MAX_RESULT_COMPONENT_BYTES: usize = 2_097_152;

/// A GPU appraisal reply from the out-of-process extension helper.
#[derive(Clone, PartialEq, Eq)]
pub struct IosAppraisalReply {
    pub returncode: i32,
    pub result_code: i32,
    pub result_message: String,
    pub claims: serde_json::Value,
    pub detached_eat: serde_json::Value,
}

impl std::fmt::Debug for IosAppraisalReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IosAppraisalReply")
            .field("returncode", &self.returncode)
            .field("result_code", &self.result_code)
            .field("result_message", &self.result_message)
            .field(
                "detached_eat_count",
                &self.detached_eat.as_array().map_or(0, Vec::len),
            )
            .finish()
    }
}

/// Events processed by [`AppraisalStateMachine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppraisalEvent {
    Start,
    Launched { pid: u32 },
    RetiredWithoutLaunch,
    Reply(IosAppraisalReply),
    Exited { pid: u32 },
    Cancel,
    Deadline,
    Malformed(String),
    Disconnect,
    SecondUse,
}

/// Terminal outcomes of an appraisal attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppraisalTerminalResult {
    Accepted(IosAppraisalReply),
    Cancelled,
    Deadline,
    Malformed(String),
    Disconnected,
    SecondUse,
    Failed(String),
}

/// Internal states of [`AppraisalStateMachine`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum MachineState {
    Initial,
    Started,
    Running {
        pid: u32,
    },
    Replied {
        pid: u32,
        reply: IosAppraisalReply,
    },
    TerminalPendingLaunch {
        result: AppraisalTerminalResult,
    },
    Terminal {
        result: AppraisalTerminalResult,
        owned_pid: Option<u32>,
        pid_exited: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppraisalStateMachineError {
    #[error("cannot start while a process is owned and running")]
    PendingOwnership,
}

/// Pure state machine enforcing single-use, ownership tracking, and drain
/// synchronization for out-of-process iOS GPU appraisal.
pub struct AppraisalStateMachine {
    state: MachineState,
}

impl Default for AppraisalStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl AppraisalStateMachine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: MachineState::Initial,
        }
    }

    /// Evaluates an event against the machine state.
    pub fn handle_event(
        &mut self,
        event: AppraisalEvent,
    ) -> Result<(), AppraisalStateMachineError> {
        // Enforce ownership check before starting
        if event == AppraisalEvent::Start {
            match &self.state {
                MachineState::Running { .. }
                | MachineState::Replied { .. }
                | MachineState::TerminalPendingLaunch { .. } => {
                    return Err(AppraisalStateMachineError::PendingOwnership);
                }
                MachineState::Terminal {
                    owned_pid: Some(_),
                    pid_exited: false,
                    ..
                } => {
                    return Err(AppraisalStateMachineError::PendingOwnership);
                }
                _ => {}
            }
        }

        if matches!(self.state, MachineState::TerminalPendingLaunch { .. }) {
            let current = std::mem::replace(&mut self.state, MachineState::Initial);
            let MachineState::TerminalPendingLaunch { result } = current else {
                unreachable!();
            };
            self.state = match event {
                AppraisalEvent::Launched { pid } => MachineState::Terminal {
                    result,
                    owned_pid: Some(pid),
                    pid_exited: false,
                },
                AppraisalEvent::RetiredWithoutLaunch => MachineState::Terminal {
                    result,
                    owned_pid: None,
                    pid_exited: true,
                },
                _ => MachineState::TerminalPendingLaunch { result },
            };
            return Ok(());
        }

        // Terminal state: result is locked and cannot be replaced.
        // Update ownership and exit tracking only.
        if let MachineState::Terminal {
            owned_pid,
            pid_exited,
            ..
        } = &mut self.state
        {
            match event {
                AppraisalEvent::Launched { pid } => {
                    if owned_pid.is_none() {
                        *owned_pid = Some(pid);
                        *pid_exited = false;
                    }
                }
                AppraisalEvent::Exited { pid } if *owned_pid == Some(pid) => {
                    *pid_exited = true;
                }
                _ => {}
            }
            return Ok(());
        }

        let current = std::mem::replace(&mut self.state, MachineState::Initial);
        self.state = match (current, event) {
            (MachineState::Initial, AppraisalEvent::Start) => MachineState::Started,
            (MachineState::Initial, AppraisalEvent::Cancel) => MachineState::Terminal {
                result: AppraisalTerminalResult::Cancelled,
                owned_pid: None,
                pid_exited: false,
            },
            (MachineState::Initial, AppraisalEvent::Deadline) => MachineState::Terminal {
                result: AppraisalTerminalResult::Deadline,
                owned_pid: None,
                pid_exited: false,
            },
            (MachineState::Initial, AppraisalEvent::SecondUse) => MachineState::Terminal {
                result: AppraisalTerminalResult::SecondUse,
                owned_pid: None,
                pid_exited: false,
            },
            (MachineState::Initial, AppraisalEvent::Disconnect) => MachineState::Terminal {
                result: AppraisalTerminalResult::Disconnected,
                owned_pid: None,
                pid_exited: false,
            },
            (MachineState::Initial, AppraisalEvent::Malformed(m)) => MachineState::Terminal {
                result: AppraisalTerminalResult::Malformed(m),
                owned_pid: None,
                pid_exited: false,
            },
            (MachineState::Initial, AppraisalEvent::Launched { pid }) => MachineState::Terminal {
                result: AppraisalTerminalResult::Failed("launched before start".into()),
                owned_pid: Some(pid),
                pid_exited: false,
            },
            (MachineState::Initial, AppraisalEvent::Exited { pid }) => MachineState::Terminal {
                result: AppraisalTerminalResult::Failed("exited before start".into()),
                owned_pid: Some(pid),
                pid_exited: true,
            },
            (MachineState::Initial, AppraisalEvent::Reply(_)) => MachineState::Terminal {
                result: AppraisalTerminalResult::Failed("reply before start".into()),
                owned_pid: None,
                pid_exited: false,
            },
            (MachineState::Initial, AppraisalEvent::RetiredWithoutLaunch) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Failed("launch retired before start".into()),
                    owned_pid: None,
                    pid_exited: true,
                }
            }

            (MachineState::Started, AppraisalEvent::Launched { pid }) => {
                MachineState::Running { pid }
            }
            (MachineState::Started, AppraisalEvent::Cancel) => {
                MachineState::TerminalPendingLaunch {
                    result: AppraisalTerminalResult::Cancelled,
                }
            }
            (MachineState::Started, AppraisalEvent::Deadline) => {
                MachineState::TerminalPendingLaunch {
                    result: AppraisalTerminalResult::Deadline,
                }
            }
            (MachineState::Started, AppraisalEvent::Disconnect) => {
                MachineState::TerminalPendingLaunch {
                    result: AppraisalTerminalResult::Disconnected,
                }
            }
            (MachineState::Started, AppraisalEvent::Malformed(m)) => {
                MachineState::TerminalPendingLaunch {
                    result: AppraisalTerminalResult::Malformed(m),
                }
            }
            (MachineState::Started, AppraisalEvent::SecondUse) => {
                MachineState::TerminalPendingLaunch {
                    result: AppraisalTerminalResult::SecondUse,
                }
            }
            (MachineState::Started, AppraisalEvent::Exited { pid }) => MachineState::Terminal {
                result: AppraisalTerminalResult::Failed("exited before launch".into()),
                owned_pid: Some(pid),
                pid_exited: true,
            },
            (MachineState::Started, AppraisalEvent::Reply(_)) => {
                MachineState::TerminalPendingLaunch {
                    result: AppraisalTerminalResult::Failed("reply before launch".into()),
                }
            }
            (MachineState::Started, AppraisalEvent::RetiredWithoutLaunch) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Failed("launch retired".into()),
                    owned_pid: None,
                    pid_exited: true,
                }
            }
            (MachineState::Started, AppraisalEvent::Start) => MachineState::Started,

            (MachineState::Running { pid }, AppraisalEvent::Reply(reply)) => {
                MachineState::Replied { pid, reply }
            }
            (MachineState::Running { pid }, AppraisalEvent::Exited { pid: exit_pid }) => {
                let exited = exit_pid == pid;
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Failed("exited before reply".into()),
                    owned_pid: Some(pid),
                    pid_exited: exited,
                }
            }
            (MachineState::Running { pid }, AppraisalEvent::Cancel) => MachineState::Terminal {
                result: AppraisalTerminalResult::Cancelled,
                owned_pid: Some(pid),
                pid_exited: false,
            },
            (MachineState::Running { pid }, AppraisalEvent::Deadline) => MachineState::Terminal {
                result: AppraisalTerminalResult::Deadline,
                owned_pid: Some(pid),
                pid_exited: false,
            },
            (MachineState::Running { pid }, AppraisalEvent::Disconnect) => MachineState::Terminal {
                result: AppraisalTerminalResult::Disconnected,
                owned_pid: Some(pid),
                pid_exited: false,
            },
            (MachineState::Running { pid }, AppraisalEvent::Malformed(m)) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Malformed(m),
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }
            (MachineState::Running { pid }, AppraisalEvent::SecondUse) => MachineState::Terminal {
                result: AppraisalTerminalResult::SecondUse,
                owned_pid: Some(pid),
                pid_exited: false,
            },
            (MachineState::Running { pid }, AppraisalEvent::Launched { .. })
            | (MachineState::Running { pid }, AppraisalEvent::Start) => {
                MachineState::Running { pid }
            }
            (MachineState::Running { pid }, AppraisalEvent::RetiredWithoutLaunch) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Failed(
                        "launch retirement reported after pid ownership".into(),
                    ),
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }

            (MachineState::Replied { pid, reply }, AppraisalEvent::Exited { pid: exit_pid }) => {
                if exit_pid == pid {
                    MachineState::Terminal {
                        result: AppraisalTerminalResult::Accepted(reply),
                        owned_pid: Some(pid),
                        pid_exited: true,
                    }
                } else {
                    MachineState::Terminal {
                        result: AppraisalTerminalResult::Failed("pid mismatch on exit".into()),
                        owned_pid: Some(pid),
                        pid_exited: false,
                    }
                }
            }
            (MachineState::Replied { pid, .. }, AppraisalEvent::Cancel) => MachineState::Terminal {
                result: AppraisalTerminalResult::Cancelled,
                owned_pid: Some(pid),
                pid_exited: false,
            },
            (MachineState::Replied { pid, .. }, AppraisalEvent::Deadline) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Deadline,
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }
            (MachineState::Replied { pid, .. }, AppraisalEvent::Disconnect) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Disconnected,
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }
            (MachineState::Replied { pid, .. }, AppraisalEvent::Malformed(m)) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Malformed(m),
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }
            (MachineState::Replied { pid, .. }, AppraisalEvent::SecondUse) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::SecondUse,
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }
            (MachineState::Replied { pid, .. }, AppraisalEvent::Reply(_)) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Failed("duplicate reply".into()),
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }
            (MachineState::Replied { pid, reply }, AppraisalEvent::Launched { .. })
            | (MachineState::Replied { pid, reply }, AppraisalEvent::Start) => {
                MachineState::Replied { pid, reply }
            }
            (MachineState::Replied { pid, .. }, AppraisalEvent::RetiredWithoutLaunch) => {
                MachineState::Terminal {
                    result: AppraisalTerminalResult::Failed(
                        "launch retirement reported after reply".into(),
                    ),
                    owned_pid: Some(pid),
                    pid_exited: false,
                }
            }

            (MachineState::TerminalPendingLaunch { .. }, _) => unreachable!(),
            (MachineState::Terminal { .. }, _) => unreachable!(),
        };

        Ok(())
    }

    /// Whether the process state has fully drained and is safe for shutdown or restart.
    ///
    /// True only when in a terminal state and either no pid was ever launched, or the
    /// launched pid has exited.
    #[must_use]
    pub fn drain_ready(&self) -> bool {
        match &self.state {
            MachineState::TerminalPendingLaunch { .. } => false,
            MachineState::Terminal {
                owned_pid,
                pid_exited,
                ..
            } => owned_pid.is_none() || *pid_exited,
            _ => false,
        }
    }

    /// Returns the terminal result if one has been reached.
    #[must_use]
    pub fn terminal_result(&self) -> Option<&AppraisalTerminalResult> {
        match &self.state {
            MachineState::TerminalPendingLaunch { result } => Some(result),
            MachineState::Terminal { result, .. } => Some(result),
            _ => None,
        }
    }
}

/// Parameters passed to the registered iOS GPU appraisal callback.
///
/// Contains NO file paths.
pub struct IosGpuAppraisalRequest<'a> {
    pub profile_fingerprint: &'a str,
    pub nonce: &'a [u8; 32],
    pub evidence_json: &'a [u8],
    pub proofs: &'a [u8],
    pub verification_time_unix: i64,
    pub budget_ms: u64,
}

impl std::fmt::Debug for IosGpuAppraisalRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IosGpuAppraisalRequest")
            .field("profile_fingerprint", &self.profile_fingerprint)
            .field("evidence_len", &self.evidence_json.len())
            .field("proofs_len", &self.proofs.len())
            .field("verification_time_unix", &self.verification_time_unix)
            .field("budget_ms", &self.budget_ms)
            .finish()
    }
}

pub type IosGpuAppraisalCallback =
    fn(&IosGpuAppraisalRequest<'_>) -> Result<IosAppraisalReply, GpuAppraisalReason>;

/// Registry holding the optional iOS GPU appraisal callback.
#[derive(Default)]
pub struct CallbackRegistry {
    callback: Option<IosGpuAppraisalCallback>,
}

impl CallbackRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self { callback: None }
    }

    pub fn register(
        &mut self,
        admitted: bool,
        callback: IosGpuAppraisalCallback,
    ) -> Result<(), IosVerifierRefusal> {
        if !admitted {
            return Err(IosVerifierRefusal::Unavailable(
                "ios verifier not admitted".into(),
            ));
        }
        if let Some(existing) = self.callback {
            if std::ptr::fn_addr_eq(existing, callback) {
                Ok(())
            } else {
                Err(IosVerifierRefusal::AlreadyInstalled)
            }
        } else {
            self.callback = Some(callback);
            Ok(())
        }
    }

    #[must_use]
    pub fn get(&self) -> Option<IosGpuAppraisalCallback> {
        self.callback
    }
}

static CALLBACK_REGISTRY: Mutex<CallbackRegistry> = Mutex::new(CallbackRegistry::new());

/// Registers the process-wide iOS GPU appraisal callback.
///
/// Succeeds only after [`admitted_ios_verifier`] succeeds. The same pointer
/// twice is Ok; a different pointer is refused with [`IosVerifierRefusal::AlreadyInstalled`].
pub fn register_ios_gpu_appraisal_callback(
    callback: IosGpuAppraisalCallback,
) -> Result<(), IosVerifierRefusal> {
    let admitted = admitted_ios_verifier().is_ok();
    let mut guard = CALLBACK_REGISTRY
        .lock()
        .map_err(|_| IosVerifierRefusal::Unavailable("callback lock poisoned".into()))?;
    guard.register(admitted, callback)
}

/// Retrieves the registered appraisal callback if installed.
#[must_use]
pub fn get_ios_gpu_appraisal_callback() -> Option<IosGpuAppraisalCallback> {
    CALLBACK_REGISTRY.lock().ok().and_then(|guard| guard.get())
}

/// Enforces concrete size and value bounds on appraisal request inputs.
pub fn validate_appraisal_request(
    profile_fingerprint: &str,
    nonce: &[u8],
    evidence_json: &[u8],
    proofs: &[u8],
    verification_time_unix: i64,
    budget_ms: u64,
) -> Result<(), GpuAppraisalReason> {
    if nonce.len() != 32 {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    if profile_fingerprint.len() != 64
        || !profile_fingerprint
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(GpuAppraisalReason::StatusProfileMissing);
    }
    if evidence_json.len() > MAX_EVIDENCE_JSON_BYTES {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    let parsed: serde_json::Value = serde_json::from_slice(evidence_json)
        .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
    if !parsed.as_array().is_some_and(|a| a.len() == 1) {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    if proofs.len() > MAX_PROOFS_BYTES {
        return Err(GpuAppraisalReason::StatusProofsMissing);
    }
    if verification_time_unix <= 0 {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    if !(1..=60_000).contains(&budget_ms) {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    Ok(())
}

/// Bridges an [`IosAppraisalReply`] through [`crate::nvgpu::classify_nvattest_result`].
pub fn classify_ios_appraisal_result(
    reply: &IosAppraisalReply,
    owner_nonce: &[u8; 32],
    verification_time_unix: i64,
) -> Result<NvattestAcceptance, GpuAppraisalReason> {
    if serde_json::to_vec(&reply.claims)
        .map_or(true, |bytes| bytes.len() > MAX_RESULT_COMPONENT_BYTES)
        || serde_json::to_vec(&reply.detached_eat)
            .map_or(true, |bytes| bytes.len() > MAX_RESULT_COMPONENT_BYTES)
        || reply.result_message.len() > 4_096
    {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    if !reply
        .detached_eat
        .as_array()
        .is_some_and(|entries| entries.len() <= MAX_DETACHED_EAT_ENTRIES)
    {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }

    let stdout_val = serde_json::json!({
        "result_code": reply.result_code,
        "result_message": reply.result_message,
        "claims": reply.claims,
        "detached_eat": reply.detached_eat,
    });

    let stdout_str =
        serde_json::to_string(&stdout_val).map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
    if stdout_str.len() > MAX_NVATTEST_STDOUT_BYTES {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }

    let expectation = StatusExpectation::OfflineSignedAge {
        verification_time_unix,
    };
    let verdict = crate::nvgpu::classify_nvattest_result(
        reply.returncode,
        &stdout_val,
        owner_nonce,
        expectation,
    );
    match verdict {
        NvattestVerdict::Accepted(acceptance) => Ok(acceptance),
        NvattestVerdict::Rejected(rejection) => Err(rejection.reason),
    }
}

fn ios_appraisal_steps(expectation: StatusExpectation) -> Vec<AppraisalStep> {
    let status = match expectation {
        StatusExpectation::OnlineNonce => "nonce-bearing OCSP",
        StatusExpectation::OfflineSignedAge { .. } => "signed-age OCSP proofs",
    };
    vec![
        AppraisalStep {
            name: "nvattest",
            status: "ok",
            detail: "returncode=0 result_code=0 result_message=Ok".to_owned(),
        },
        AppraisalStep {
            name: "overall-eat",
            status: "ok",
            detail: "alg=none iss=NVAT-LOCAL-VERIFIER overall_att_result=True".to_owned(),
        },
        AppraisalStep {
            name: "gpu-claims",
            status: "ok",
            detail: format!(
                "claims-version=3.0 report, driver-RIM, vbios-RIM checks passed; status={status}"
            ),
        },
    ]
}

/// Executes bounded iOS GPU appraisal via the registered transport callback.
pub fn appraise_ios_gpu(
    envelope: &GpuEnvelope,
    owner_nonce: &[u8; 32],
    profile: &GpuProfile,
    proofs: Option<&[u8]>,
    verification_time: SystemTime,
) -> Result<GpuAppraisal, GpuAppraisalReason> {
    // ManifestSet equality is pointer identity. Const promotion can give the
    // same qualified set a different address across code generation units.
    // Compare the complete pinned contents at this platform boundary.
    if !crate::nvgpu::is_production_manifest_set(profile.manifests()) {
        return Err(GpuAppraisalReason::NvattestUnavailable);
    }
    if profile.status() == StatusMode::OnlineNonce {
        return Err(GpuAppraisalReason::NvattestUnavailable);
    }
    let proofs = match proofs {
        Some(p) if !p.is_empty() => p,
        _ => return Err(GpuAppraisalReason::StatusProofsMissing),
    };

    let verification_time_unix = verification_time
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .filter(|seconds| *seconds > 0)
        .ok_or(GpuAppraisalReason::GpuAppraisalFailed)?;

    let evidence_val = evidence_json(envelope, owner_nonce)?;
    let evidence_bytes =
        serde_json::to_vec(&evidence_val).map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;

    validate_appraisal_request(
        profile.pcr_sha256(),
        owner_nonce,
        &evidence_bytes,
        proofs,
        verification_time_unix,
        60_000,
    )?;

    let callback =
        get_ios_gpu_appraisal_callback().ok_or(GpuAppraisalReason::NvattestUnavailable)?;

    let request = IosGpuAppraisalRequest {
        profile_fingerprint: profile.pcr_sha256(),
        nonce: owner_nonce,
        evidence_json: &evidence_bytes,
        proofs,
        verification_time_unix,
        budget_ms: 60_000,
    };

    let reply = callback(&request)?;

    let acceptance = classify_ios_appraisal_result(&reply, owner_nonce, verification_time_unix)?;
    let steps = ios_appraisal_steps(StatusExpectation::OfflineSignedAge {
        verification_time_unix,
    });
    crate::nvgpu::build_gpu_appraisal(&acceptance.claim, envelope, steps, acceptance.status)
        .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_reply() -> IosAppraisalReply {
        IosAppraisalReply {
            returncode: 0,
            result_code: 0,
            result_message: "Ok".into(),
            claims: serde_json::json!([{"x-nvidia-overall-result": true}]),
            detached_eat: serde_json::json!([["JWT", "fake-jwt"]]),
        }
    }

    #[test]
    fn state_machine_normal_flow_accepts_reply_after_exit() {
        let mut sm = AppraisalStateMachine::new();
        assert!(!sm.drain_ready());

        sm.handle_event(AppraisalEvent::Start).unwrap();
        sm.handle_event(AppraisalEvent::Launched { pid: 1234 })
            .unwrap();
        assert!(!sm.drain_ready());

        sm.handle_event(AppraisalEvent::Reply(dummy_reply()))
            .unwrap();
        assert!(!sm.drain_ready());
        assert!(sm.terminal_result().is_none());

        sm.handle_event(AppraisalEvent::Exited { pid: 1234 })
            .unwrap();
        assert!(sm.drain_ready());
        assert!(matches!(
            sm.terminal_result(),
            Some(AppraisalTerminalResult::Accepted(_))
        ));
    }

    #[test]
    fn state_machine_exit_before_reply_fails() {
        let mut sm = AppraisalStateMachine::new();
        sm.handle_event(AppraisalEvent::Start).unwrap();
        sm.handle_event(AppraisalEvent::Launched { pid: 1234 })
            .unwrap();
        sm.handle_event(AppraisalEvent::Exited { pid: 1234 })
            .unwrap();

        assert!(sm.drain_ready());
        assert!(matches!(
            sm.terminal_result(),
            Some(AppraisalTerminalResult::Failed(_))
        ));
    }

    #[test]
    fn state_machine_pid_mismatch_fails_and_does_not_drain() {
        let mut sm = AppraisalStateMachine::new();
        sm.handle_event(AppraisalEvent::Start).unwrap();
        sm.handle_event(AppraisalEvent::Launched { pid: 1234 })
            .unwrap();
        sm.handle_event(AppraisalEvent::Reply(dummy_reply()))
            .unwrap();
        sm.handle_event(AppraisalEvent::Exited { pid: 9999 })
            .unwrap();

        assert!(!sm.drain_ready());
        assert!(matches!(
            sm.terminal_result(),
            Some(AppraisalTerminalResult::Failed(_))
        ));
    }

    #[test]
    fn state_machine_cancel_before_launch_retains_pending_activation() {
        let mut sm = AppraisalStateMachine::new();
        sm.handle_event(AppraisalEvent::Start).unwrap();
        sm.handle_event(AppraisalEvent::Cancel).unwrap();

        assert!(!sm.drain_ready());
        assert_eq!(
            sm.terminal_result(),
            Some(&AppraisalTerminalResult::Cancelled)
        );

        // Later launch after cancel: retains ownership until exit
        sm.handle_event(AppraisalEvent::Launched { pid: 5678 })
            .unwrap();
        assert!(!sm.drain_ready());
        assert_eq!(
            sm.terminal_result(),
            Some(&AppraisalTerminalResult::Cancelled)
        );

        sm.handle_event(AppraisalEvent::Exited { pid: 5678 })
            .unwrap();
        assert!(sm.drain_ready());

        let mut retired = AppraisalStateMachine::new();
        retired.handle_event(AppraisalEvent::Start).unwrap();
        retired.handle_event(AppraisalEvent::Cancel).unwrap();
        retired
            .handle_event(AppraisalEvent::RetiredWithoutLaunch)
            .unwrap();
        assert!(retired.drain_ready());
        assert_eq!(
            retired.terminal_result(),
            Some(&AppraisalTerminalResult::Cancelled)
        );
    }

    #[test]
    fn state_machine_refuses_start_when_process_is_owned_and_not_exited() {
        let mut sm = AppraisalStateMachine::new();
        sm.handle_event(AppraisalEvent::Start).unwrap();
        sm.handle_event(AppraisalEvent::Launched { pid: 1234 })
            .unwrap();

        let err = sm.handle_event(AppraisalEvent::Start).unwrap_err();
        assert_eq!(err, AppraisalStateMachineError::PendingOwnership);
    }

    #[test]
    fn validate_request_bounds_enforced() {
        let valid_profile = "a".repeat(64);
        let valid_nonce = [1u8; 32];
        let valid_evidence = b"[{\"valid\": true}]";
        let valid_proofs = vec![0u8; 100];

        // Valid case
        assert!(
            validate_appraisal_request(
                &valid_profile,
                &valid_nonce,
                valid_evidence,
                &valid_proofs,
                1_700_000_000,
                30_000
            )
            .is_ok()
        );

        // Nonce length != 32
        assert!(
            validate_appraisal_request(
                &valid_profile,
                &[1u8; 16],
                valid_evidence,
                &valid_proofs,
                1_700_000_000,
                30_000
            )
            .is_err()
        );

        // Profile length != 64 or non-hex
        assert!(
            validate_appraisal_request(
                "short",
                &valid_nonce,
                valid_evidence,
                &valid_proofs,
                1_700_000_000,
                30_000
            )
            .is_err()
        );

        // Evidence JSON not a 1-element array
        assert!(
            validate_appraisal_request(
                &valid_profile,
                &valid_nonce,
                b"{\"not\":\"array\"}",
                &valid_proofs,
                1_700_000_000,
                30_000
            )
            .is_err()
        );

        // Verification time <= 0
        assert!(
            validate_appraisal_request(
                &valid_profile,
                &valid_nonce,
                valid_evidence,
                &valid_proofs,
                0,
                30_000
            )
            .is_err()
        );

        // Budget out of bounds
        assert!(
            validate_appraisal_request(
                &valid_profile,
                &valid_nonce,
                valid_evidence,
                &valid_proofs,
                1_700_000_000,
                0
            )
            .is_err()
        );
        assert!(
            validate_appraisal_request(
                &valid_profile,
                &valid_nonce,
                valid_evidence,
                &valid_proofs,
                1_700_000_000,
                70_000
            )
            .is_err()
        );
    }

    fn dummy_cb1(
        _req: &IosGpuAppraisalRequest<'_>,
    ) -> Result<IosAppraisalReply, GpuAppraisalReason> {
        Err(GpuAppraisalReason::GpuAppraisalFailed)
    }

    fn dummy_cb2(
        _req: &IosGpuAppraisalRequest<'_>,
    ) -> Result<IosAppraisalReply, GpuAppraisalReason> {
        Err(GpuAppraisalReason::NvattestUnavailable)
    }

    #[test]
    fn callback_registry_enforces_rules() {
        let mut registry = CallbackRegistry::new();

        // 1. Register when not admitted fails
        let err = registry.register(false, dummy_cb1).unwrap_err();
        assert!(matches!(err, IosVerifierRefusal::Unavailable(_)));
        assert!(registry.get().is_none());

        // 2. Register first pointer when admitted succeeds
        registry.register(true, dummy_cb1).expect("register cb1");
        assert!(registry.get().is_some());

        // 3. Same pointer registered again succeeds
        registry
            .register(true, dummy_cb1)
            .expect("register cb1 again");

        // 4. Different pointer is refused with AlreadyInstalled
        let err2 = registry.register(true, dummy_cb2).unwrap_err();
        assert_eq!(err2, IosVerifierRefusal::AlreadyInstalled);
    }

    #[test]
    fn state_machine_cancel_and_deadline_lock_terminal_across_reply_and_exit() {
        // Cancel path
        let mut sm_cancel = AppraisalStateMachine::new();
        sm_cancel.handle_event(AppraisalEvent::Start).unwrap();
        sm_cancel
            .handle_event(AppraisalEvent::Launched { pid: 42 })
            .unwrap();
        sm_cancel.handle_event(AppraisalEvent::Cancel).unwrap();
        assert!(!sm_cancel.drain_ready());
        assert_eq!(
            sm_cancel.terminal_result(),
            Some(&AppraisalTerminalResult::Cancelled)
        );

        sm_cancel
            .handle_event(AppraisalEvent::Reply(dummy_reply()))
            .unwrap();
        assert!(!sm_cancel.drain_ready());
        assert_eq!(
            sm_cancel.terminal_result(),
            Some(&AppraisalTerminalResult::Cancelled)
        );

        sm_cancel
            .handle_event(AppraisalEvent::Exited { pid: 42 })
            .unwrap();
        assert!(sm_cancel.drain_ready());
        assert_eq!(
            sm_cancel.terminal_result(),
            Some(&AppraisalTerminalResult::Cancelled)
        );

        // Deadline path
        let mut sm_deadline = AppraisalStateMachine::new();
        sm_deadline.handle_event(AppraisalEvent::Start).unwrap();
        sm_deadline
            .handle_event(AppraisalEvent::Launched { pid: 43 })
            .unwrap();
        sm_deadline.handle_event(AppraisalEvent::Deadline).unwrap();
        assert!(!sm_deadline.drain_ready());
        assert_eq!(
            sm_deadline.terminal_result(),
            Some(&AppraisalTerminalResult::Deadline)
        );

        sm_deadline
            .handle_event(AppraisalEvent::Reply(dummy_reply()))
            .unwrap();
        assert!(!sm_deadline.drain_ready());
        assert_eq!(
            sm_deadline.terminal_result(),
            Some(&AppraisalTerminalResult::Deadline)
        );

        sm_deadline
            .handle_event(AppraisalEvent::Exited { pid: 43 })
            .unwrap();
        assert!(sm_deadline.drain_ready());
        assert_eq!(
            sm_deadline.terminal_result(),
            Some(&AppraisalTerminalResult::Deadline)
        );
    }

    #[test]
    fn classify_result_code_504_maps_to_gpu_nonce_mismatch() {
        let reply = IosAppraisalReply {
            returncode: 0,
            result_code: 504,
            result_message: "Nonce mismatch in attestation".into(),
            claims: serde_json::json!([]),
            detached_eat: serde_json::json!([]),
        };
        let nonce = [0u8; 32];
        let err = classify_ios_appraisal_result(&reply, &nonce, 1_700_000_000).unwrap_err();
        assert_eq!(err, GpuAppraisalReason::GpuNonceMismatch);
    }
}
