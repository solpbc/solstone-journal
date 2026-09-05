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
    ProviderAlreadyConfigured,
    ProviderConfigUnexpectedShape,
    LocalProviderUnavailable,
    /// This reason accompanies a brain `warning`, not a skipped result.
    LocalBootstrapDidNotStart,
    SolAlreadyKeepsJournal,
    PriorRunOk,
    /// This reason accompanies a service `ok` result, not a skipped result.
    ResumedAfterRestart,
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
            Self::ProviderAlreadyConfigured => "a provider is already configured",
            Self::ProviderConfigUnexpectedShape => "provider config is not in the expected shape",
            Self::LocalProviderUnavailable => "local provider unavailable on this host",
            Self::LocalBootstrapDidNotStart => "local bootstrap did not start",
            Self::SolAlreadyKeepsJournal => "the journal already lives on this mac",
            Self::PriorRunOk => "prior_run_ok",
            Self::ResumedAfterRestart => "resumed_after_restart",
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
            SkipReason::SolAlreadyKeepsJournal,
            SkipReason::PriorRunOk,
            SkipReason::ResumedAfterRestart,
        ];
        assert_eq!(events.len(), 9);
        assert_eq!(steps.len(), 8);
        assert_eq!(errors.len(), 9);
        assert_eq!(reasons.len(), 13);
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
}
