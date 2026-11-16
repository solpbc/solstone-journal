// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed JSONL vocabulary shared by native setup and doctor forwarding.

use std::io::{self, Write};

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EventType {
    #[serde(rename = "setup.started")]
    SetupStarted,
    #[serde(rename = "setup.completed")]
    SetupCompleted,
    #[serde(rename = "step.started")]
    StepStarted,
    #[serde(rename = "step.completed")]
    StepCompleted,
    #[serde(rename = "step.failed")]
    StepFailed,
    #[serde(rename = "step.warning")]
    StepWarning,
    #[serde(rename = "doctor.started")]
    DoctorStarted,
    #[serde(rename = "check.completed")]
    CheckCompleted,
    #[serde(rename = "doctor.completed")]
    DoctorCompleted,
}

impl EventType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SetupStarted => "setup.started",
            Self::SetupCompleted => "setup.completed",
            Self::StepStarted => "step.started",
            Self::StepCompleted => "step.completed",
            Self::StepFailed => "step.failed",
            Self::StepWarning => "step.warning",
            Self::DoctorStarted => "doctor.started",
            Self::CheckCompleted => "check.completed",
            Self::DoctorCompleted => "doctor.completed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StepName {
    #[serde(rename = "doctor")]
    Doctor,
    #[serde(rename = "journal")]
    Journal,
    #[serde(rename = "install_models")]
    InstallModels,
    #[serde(rename = "skills_user")]
    SkillsUser,
    #[serde(rename = "skills_journal")]
    SkillsJournal,
    #[serde(rename = "wrapper")]
    Wrapper,
    #[serde(rename = "service")]
    Service,
    #[serde(rename = "brain")]
    Brain,
}

impl StepName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Doctor => "doctor",
            Self::Journal => "journal",
            Self::InstallModels => "install_models",
            Self::SkillsUser => "skills_user",
            Self::SkillsJournal => "skills_journal",
            Self::Wrapper => "wrapper",
            Self::Service => "service",
            Self::Brain => "brain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ErrorCode {
    #[serde(rename = "doctor_failed")]
    DoctorFailed,
    #[serde(rename = "doctor_jsonl_incomplete")]
    DoctorJsonlIncomplete,
    #[serde(rename = "doctor_timeout")]
    DoctorTimeout,
    #[serde(rename = "journal_dir_invalid")]
    JournalDirInvalid,
    #[serde(rename = "journal_existing_blocked")]
    JournalExistingBlocked,
    #[serde(rename = "installation_identity_refused")]
    InstallationIdentityRefused,
    #[serde(rename = "installation_identity_unavailable")]
    InstallationIdentityUnavailable,
    #[serde(rename = "legacy_supervisor_lock_refused")]
    LegacySupervisorLockRefused,
    #[serde(rename = "service_up_failed")]
    ServiceUpFailed,
    #[serde(rename = "setup_unhandled_exception")]
    SetupUnhandledException,
    #[serde(rename = "step_subprocess_failed")]
    StepSubprocessFailed,
    #[serde(rename = "step_subprocess_timeout")]
    StepSubprocessTimeout,
    #[serde(rename = "wrapper_provision_failed")]
    WrapperProvisionFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    SkipModels,
    SkipBrain,
    SkipModelsImpliesSkipBrain,
    SkipSkills,
    SkipService,
    SkipWrapper,
    WindowsPackageOwnsCommands,
    /// A mac gets no persistent command on its PATH: the journal app's admin
    /// terminal provides `journal` and `solstone`.
    MacAppOwnsCommands,
    /// Agent skills install through symlinks, which a standard Windows owner
    /// cannot create; the step is skipped rather than reported as a failure.
    WindowsAgentSkillsUnavailable,
    ProviderAlreadyConfigured,
    ProviderConfigUnexpectedShape,
    LocalProviderUnavailable,
    /// This reason accompanies a brain `warning`, not a skipped result.
    LocalBootstrapDidNotStart,
    /// The journal exists on disk but the owner has not created it yet, so
    /// local thinking is installed from the Thinking page afterwards.
    JournalNotCreatedYet,
    SolAlreadyKeepsJournal,
    PriorRunOk,
    /// This reason accompanies a service `ok` result, not a skipped result.
    ResumedAfterRestart,
    /// An automatic update installs the new journal beside the running one
    /// and leaves the service as it is: the new version starts the next time
    /// the journal does.
    UnattendedUpdate,
}

impl SkipReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SkipModels => "--skip-models",
            Self::SkipBrain => "--skip-brain",
            Self::SkipModelsImpliesSkipBrain => "--skip-models implies --skip-brain",
            Self::SkipSkills => "--skip-skills",
            Self::SkipService => "--skip-service",
            Self::SkipWrapper => "--skip-wrapper",
            Self::WindowsPackageOwnsCommands => {
                "Windows packages expose the commands directly; POSIX wrappers are not applicable"
            }
            Self::MacAppOwnsCommands => {
                "the journal app's admin terminal provides the commands on a mac; no wrappers are written"
            }
            Self::WindowsAgentSkillsUnavailable => "agent skills are not installed on windows yet",
            Self::ProviderAlreadyConfigured => "a provider is already configured",
            Self::ProviderConfigUnexpectedShape => "provider config is not in the expected shape",
            Self::LocalProviderUnavailable => "local provider unavailable on this host",
            Self::LocalBootstrapDidNotStart => "local bootstrap did not start",
            Self::JournalNotCreatedYet => {
                "choose \"install local model\" on the thinking page once you've created your journal"
            }
            Self::SolAlreadyKeepsJournal => "the journal already lives on this mac",
            Self::PriorRunOk => "prior_run_ok",
            Self::ResumedAfterRestart => "resumed_after_restart",
            Self::UnattendedUpdate => "unattended_update",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

#[must_use]
pub const fn translate_status(status: ForeignStatus) -> &'static str {
    match status {
        ForeignStatus::Ok => "ok",
        ForeignStatus::Warn => "warning",
        ForeignStatus::Fail => "failed",
        ForeignStatus::Skip => "skipped",
    }
}

/// Emits one compact JSON object per line.
pub struct JsonlEmitter<W> {
    writer: W,
}

/// Common event sink for setup's run loop and test recorders.
pub trait EventSink {
    fn emit(
        &mut self,
        event: EventType,
        timestamp: &str,
        fields: Map<String, Value>,
    ) -> io::Result<()>;
    fn forward_line(&mut self, line: &str) -> io::Result<()>;
}

impl<W: Write> JsonlEmitter<W> {
    #[must_use]
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn emit(
        &mut self,
        event: EventType,
        timestamp: &str,
        fields: Map<String, Value>,
    ) -> io::Result<()> {
        let mut payload = Map::new();
        payload.insert("event".to_owned(), Value::String(event.as_str().to_owned()));
        payload.insert("ts".to_owned(), Value::String(timestamp.to_owned()));
        payload.extend(fields);
        serde_json::to_writer(&mut self.writer, &Value::Object(payload))
            .map_err(io::Error::other)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    pub fn forward_line(&mut self, line: &str) -> io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        if !line.ends_with('\n') {
            self.writer.write_all(b"\n")?;
        }
        self.writer.flush()
    }

    #[must_use]
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> EventSink for JsonlEmitter<W> {
    fn emit(
        &mut self,
        event: EventType,
        timestamp: &str,
        fields: Map<String, Value>,
    ) -> io::Result<()> {
        Self::emit(self, event, timestamp, fields)
    }

    fn forward_line(&mut self, line: &str) -> io::Result<()> {
        Self::forward_line(self, line)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ErrorCode, EventType, ForeignStatus, JsonlEmitter, SkipReason, StepName, translate_status,
    };
    use serde_json::{Map, json};

    #[test]
    fn closed_vocabularies_have_the_reference_sizes() {
        let events = [
            EventType::SetupStarted,
            EventType::SetupCompleted,
            EventType::StepStarted,
            EventType::StepCompleted,
            EventType::StepFailed,
            EventType::StepWarning,
            EventType::DoctorStarted,
            EventType::CheckCompleted,
            EventType::DoctorCompleted,
        ];
        let steps = [
            StepName::Doctor,
            StepName::Journal,
            StepName::InstallModels,
            StepName::SkillsUser,
            StepName::SkillsJournal,
            StepName::Wrapper,
            StepName::Service,
            StepName::Brain,
        ];
        let errors = [
            ErrorCode::DoctorFailed,
            ErrorCode::DoctorJsonlIncomplete,
            ErrorCode::DoctorTimeout,
            ErrorCode::JournalDirInvalid,
            ErrorCode::JournalExistingBlocked,
            ErrorCode::ServiceUpFailed,
            ErrorCode::SetupUnhandledException,
            ErrorCode::StepSubprocessFailed,
            ErrorCode::StepSubprocessTimeout,
        ];
        let reasons = [
            SkipReason::SkipModels,
            SkipReason::SkipBrain,
            SkipReason::SkipModelsImpliesSkipBrain,
            SkipReason::SkipSkills,
            SkipReason::SkipService,
            SkipReason::SkipWrapper,
            SkipReason::ProviderAlreadyConfigured,
            SkipReason::ProviderConfigUnexpectedShape,
            SkipReason::LocalProviderUnavailable,
            SkipReason::LocalBootstrapDidNotStart,
            SkipReason::JournalNotCreatedYet,
            SkipReason::SolAlreadyKeepsJournal,
            SkipReason::PriorRunOk,
            SkipReason::ResumedAfterRestart,
        ];
        assert_eq!(events.len(), 9);
        assert_eq!(steps.len(), 8);
        assert_eq!(errors.len(), 9);
        assert_eq!(reasons.len(), 14);
        assert_eq!(
            SkipReason::LocalBootstrapDidNotStart.as_str(),
            "local bootstrap did not start"
        );
        assert_eq!(
            SkipReason::ResumedAfterRestart.as_str(),
            "resumed_after_restart"
        );
    }

    #[test]
    fn status_translation_and_jsonl_emission_match_the_contract() {
        assert_eq!(translate_status(ForeignStatus::Ok), "ok");
        assert_eq!(translate_status(ForeignStatus::Warn), "warning");
        assert_eq!(translate_status(ForeignStatus::Fail), "failed");
        assert_eq!(translate_status(ForeignStatus::Skip), "skipped");
        let mut emitter = JsonlEmitter::new(Vec::new());
        let mut fields = Map::new();
        fields.insert("step".into(), json!("journal"));
        emitter
            .emit(EventType::StepStarted, "2026-01-01T00:00:00Z", fields)
            .unwrap();
        assert_eq!(
            String::from_utf8(emitter.into_inner()).unwrap(),
            "{\"event\":\"step.started\",\"ts\":\"2026-01-01T00:00:00Z\",\"step\":\"journal\"}\n"
        );
    }

    /// The Journal Mac app (platforms/macos) reads setup's JSONL. This fixture is
    /// what it promises to understand: every value of the closed vocabularies and
    /// one line per event type as this crate's emitter writes it. The Swift tests
    /// decode it; this test keeps it equal to the native authority. Regenerate with
    /// `SOLSTONE_UPDATE_SETUP_CONTRACT=1 cargo test -p solstone-core-setup` and
    /// commit the result.
    #[test]
    fn mac_app_setup_contract_fixture_matches_the_native_vocabulary() {
        use serde_json::Value;

        fn every<T: Copy>(values: &[T], exhaustive: impl Fn(T)) -> Vec<T> {
            values.iter().copied().for_each(&exhaustive);
            values.to_vec()
        }
        // The matches have no wildcard arm: a new variant fails to compile here
        // until it is listed, so the fixture cannot silently fall behind.
        let events = every(
            &[
                EventType::SetupStarted,
                EventType::SetupCompleted,
                EventType::StepStarted,
                EventType::StepCompleted,
                EventType::StepFailed,
                EventType::StepWarning,
                EventType::DoctorStarted,
                EventType::CheckCompleted,
                EventType::DoctorCompleted,
            ],
            |event| match event {
                EventType::SetupStarted
                | EventType::SetupCompleted
                | EventType::StepStarted
                | EventType::StepCompleted
                | EventType::StepFailed
                | EventType::StepWarning
                | EventType::DoctorStarted
                | EventType::CheckCompleted
                | EventType::DoctorCompleted => {}
            },
        );
        let steps = every(
            &[
                StepName::Doctor,
                StepName::Journal,
                StepName::InstallModels,
                StepName::SkillsUser,
                StepName::SkillsJournal,
                StepName::Wrapper,
                StepName::Service,
                StepName::Brain,
            ],
            |step| match step {
                StepName::Doctor
                | StepName::Journal
                | StepName::InstallModels
                | StepName::SkillsUser
                | StepName::SkillsJournal
                | StepName::Wrapper
                | StepName::Service
                | StepName::Brain => {}
            },
        );
        let errors = every(
            &[
                ErrorCode::DoctorFailed,
                ErrorCode::DoctorJsonlIncomplete,
                ErrorCode::DoctorTimeout,
                ErrorCode::JournalDirInvalid,
                ErrorCode::JournalExistingBlocked,
                ErrorCode::InstallationIdentityRefused,
                ErrorCode::InstallationIdentityUnavailable,
                ErrorCode::LegacySupervisorLockRefused,
                ErrorCode::ServiceUpFailed,
                ErrorCode::SetupUnhandledException,
                ErrorCode::StepSubprocessFailed,
                ErrorCode::StepSubprocessTimeout,
                ErrorCode::WrapperProvisionFailed,
            ],
            |code| match code {
                ErrorCode::DoctorFailed
                | ErrorCode::DoctorJsonlIncomplete
                | ErrorCode::DoctorTimeout
                | ErrorCode::JournalDirInvalid
                | ErrorCode::JournalExistingBlocked
                | ErrorCode::InstallationIdentityRefused
                | ErrorCode::InstallationIdentityUnavailable
                | ErrorCode::LegacySupervisorLockRefused
                | ErrorCode::ServiceUpFailed
                | ErrorCode::SetupUnhandledException
                | ErrorCode::StepSubprocessFailed
                | ErrorCode::StepSubprocessTimeout
                | ErrorCode::WrapperProvisionFailed => {}
            },
        );
        let reasons = every(
            &[
                SkipReason::SkipModels,
                SkipReason::SkipBrain,
                SkipReason::SkipModelsImpliesSkipBrain,
                SkipReason::SkipSkills,
                SkipReason::SkipService,
                SkipReason::SkipWrapper,
                SkipReason::WindowsPackageOwnsCommands,
                SkipReason::MacAppOwnsCommands,
                SkipReason::WindowsAgentSkillsUnavailable,
                SkipReason::ProviderAlreadyConfigured,
                SkipReason::ProviderConfigUnexpectedShape,
                SkipReason::LocalProviderUnavailable,
                SkipReason::LocalBootstrapDidNotStart,
                SkipReason::JournalNotCreatedYet,
                SkipReason::SolAlreadyKeepsJournal,
                SkipReason::PriorRunOk,
                SkipReason::ResumedAfterRestart,
                SkipReason::UnattendedUpdate,
            ],
            |reason| match reason {
                SkipReason::SkipModels
                | SkipReason::SkipBrain
                | SkipReason::SkipModelsImpliesSkipBrain
                | SkipReason::SkipSkills
                | SkipReason::SkipService
                | SkipReason::SkipWrapper
                | SkipReason::WindowsPackageOwnsCommands
                | SkipReason::MacAppOwnsCommands
                | SkipReason::WindowsAgentSkillsUnavailable
                | SkipReason::ProviderAlreadyConfigured
                | SkipReason::ProviderConfigUnexpectedShape
                | SkipReason::LocalProviderUnavailable
                | SkipReason::LocalBootstrapDidNotStart
                | SkipReason::JournalNotCreatedYet
                | SkipReason::SolAlreadyKeepsJournal
                | SkipReason::PriorRunOk
                | SkipReason::ResumedAfterRestart
                | SkipReason::UnattendedUpdate => {}
            },
        );
        let statuses: Vec<&str> = [
            ForeignStatus::Ok,
            ForeignStatus::Warn,
            ForeignStatus::Fail,
            ForeignStatus::Skip,
        ]
        .into_iter()
        .map(translate_status)
        .collect();

        let ts = "2026-01-01T00:00:00Z";
        let mut emitter = JsonlEmitter::new(Vec::new());
        for event in &events {
            let fields = match event {
                EventType::SetupStarted => {
                    json!({"started_at": ts, "version": "2.0.0", "mode": "non_interactive"})
                }
                EventType::SetupCompleted => {
                    json!({"status": "failed", "failed_step": "journal", "duration_ms": 12})
                }
                EventType::StepStarted => json!({"step": "journal", "index": 2, "total": 8}),
                EventType::StepCompleted => {
                    json!({"step": "wrapper", "outcome": "skipped", "reason": SkipReason::MacAppOwnsCommands.as_str(), "duration_ms": 0})
                }
                EventType::StepFailed => {
                    json!({"step": "journal", "duration_ms": 0, "error": {"code": ErrorCode::JournalDirInvalid, "message": "journal directory is not usable", "details": "", "exit_code": 1}})
                }
                EventType::StepWarning => {
                    json!({"step": "brain", "text": "local bootstrap did not start", "fix_hint": "install the local model from the thinking page"})
                }
                EventType::DoctorStarted => json!({"version": "2.0.0"}),
                EventType::CheckCompleted => {
                    json!({"name": "disk_space", "severity": "advisory", "status": "warning", "detail": "low disk space", "fix": ""})
                }
                EventType::DoctorCompleted => {
                    json!({"status": "warning", "duration_ms": 42, "summary": {"total": 17, "failed": 0, "warnings": 1, "skipped": 2}})
                }
            };
            let Value::Object(fields) = fields else {
                unreachable!()
            };
            emitter.emit(*event, ts, fields).unwrap();
        }
        let lines: Vec<String> = String::from_utf8(emitter.into_inner())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();

        let contract = json!({
            "schema_version": 1,
            "authority": "core/crates/solstone-core-setup/src/events.rs",
            "event_types": events.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
            "step_names": steps.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            "error_codes": errors,
            "skip_reasons": reasons.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
            "statuses": statuses,
            "lines": lines,
        });
        let rendered = serde_json::to_string_pretty(&contract).unwrap() + "\n";
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../platforms/macos/Tests/JournalRuntimeTests/Fixtures/native-setup-contract.json");
        if std::env::var_os("SOLSTONE_UPDATE_SETUP_CONTRACT").is_some() {
            std::fs::write(&fixture, &rendered).unwrap();
        }
        let committed = std::fs::read_to_string(&fixture).unwrap_or_default();
        assert_eq!(
            committed, rendered,
            "the Mac app's setup contract fixture is stale; regenerate it with \
             SOLSTONE_UPDATE_SETUP_CONTRACT=1 cargo test -p solstone-core-setup"
        );
    }
}
