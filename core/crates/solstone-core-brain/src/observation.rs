// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumDiagnosticEmitter;

use crate::{BrainRefreshPermit, WriterError, fixture::local_contract};

#[derive(Clone)]
pub struct BrainObservationSink(CallosumDiagnosticEmitter);

impl BrainObservationSink {
    pub fn new(emitter: CallosumDiagnosticEmitter) -> Self {
        Self(emitter)
    }

    pub fn begin(&self, caller: ObservationCaller) -> Option<BrainObservation> {
        let mut bytes = [0_u8; 16];
        if getrandom::fill(&mut bytes).is_err() {
            self.0.missing_brain_identity();
            return None;
        }
        let mut id = String::new();
        if id.try_reserve_exact(32).is_err() {
            self.0.missing_brain_identity();
            return None;
        }
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            id.push(char::from(HEX[usize::from(byte >> 4)]));
            id.push(char::from(HEX[usize::from(byte & 15)]));
        }
        Some(BrainObservation {
            sink: self.clone(),
            id,
            caller,
        })
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationCaller {
    MobileRefresh,
    MobileRenewal,
    ScheduledRefresh,
    Generate,
    Transcription,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationPhase {
    Admission,
    Permit,
    Finish,
    Abandon,
    Renewal,
    CredentialSettlement,
    RootBinding,
    Terminal,
    RuntimeReadiness,
    RuntimeTarget,
    RuntimeEstablish,
    TranscriptionReattest,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationOutcome {
    Admitted,
    Published,
    Ready,
    NonReady,
    Busy,
    StaleExpectedFingerprint,
    LostFence,
    OperationalFailure,
    Cancelled,
    Unchanged,
    Refused,
    PublicationUncertain,
    ExecutionUnavailable,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationFailure {
    Io,
    PublicationUncertain,
    Config,
    Validation,
    ConflictOther,
    Fingerprint,
    Random,
    Cancelled,
    CredentialChanged,
    PermitCheckpointFailed,
    CredentialSettlementFailed,
    RootBindingFailed,
    PermitExpired,
    CheckingAbsent,
    RecordRevisionChanged,
    RunChanged,
    CheckingRevisionChanged,
    RuntimeMarkerChanged,
    RootChanged,
    RecomputedFingerprint,
    CredentialBinding,
    StateUnavailable,
    KeyUnavailable,
    FingerprintUnavailable,
    FingerprintMismatch,
    ReasonNotRecordable,
    ComponentNotAllowed,
    UnknownRefusal,
}

impl ObservationFailure {
    pub fn writer(error: &WriterError) -> Self {
        match error {
            WriterError::Io(_) => Self::Io,
            WriterError::PublicationUncertain(_) => Self::PublicationUncertain,
            WriterError::Config(_) => Self::Config,
            WriterError::Validation(_) => Self::Validation,
            WriterError::Fingerprint(_) => Self::Fingerprint,
            WriterError::Random => Self::Random,
            WriterError::Cancelled(_) => Self::Cancelled,
            WriterError::Conflict(message) => match message.as_str() {
                "credential rejection changed during refresh" => Self::CredentialChanged,
                "brain refresh permit expired" => Self::PermitExpired,
                "brain refresh checking marker is absent" => Self::CheckingAbsent,
                "brain refresh record revision changed" => Self::RecordRevisionChanged,
                "brain refresh run id changed" => Self::RunChanged,
                "brain refresh revision changed" => Self::CheckingRevisionChanged,
                "brain runtime failure marker changed" => Self::RuntimeMarkerChanged,
                _ => Self::ConflictOther,
            },
        }
    }
}

#[derive(Clone)]
pub struct BrainObservation {
    sink: BrainObservationSink,
    id: String,
    caller: ObservationCaller,
}

impl BrainObservation {
    pub fn id(&self) -> &str {
        &self.id
    }

    fn emit(
        &self,
        phase: ObservationPhase,
        outcome: ObservationOutcome,
        extra: Map<String, Value>,
    ) {
        let mut fields = Map::from_iter([
            ("observation_id".into(), json!(self.id)),
            ("caller".into(), json!(self.caller)),
            ("phase".into(), json!(phase)),
            ("outcome".into(), json!(outcome)),
        ]);
        fields.extend(extra);
        let _ = self.sink.0.emit("supervisor", "brain_observation", fields);
    }

    pub fn outcome(&self, phase: ObservationPhase, outcome: ObservationOutcome) {
        self.emit(phase, outcome, Map::new());
    }

    pub fn failure(&self, phase: ObservationPhase, failure: ObservationFailure) {
        self.emit(
            phase,
            ObservationOutcome::Refused,
            Map::from_iter([("failure_category".into(), json!(failure))]),
        );
    }

    pub fn writer_failure(&self, phase: ObservationPhase, error: &WriterError) {
        self.emit(
            phase,
            if matches!(error, WriterError::PublicationUncertain(_)) {
                ObservationOutcome::PublicationUncertain
            } else {
                ObservationOutcome::Refused
            },
            Map::from_iter([(
                "failure_category".into(),
                json!(ObservationFailure::writer(error)),
            )]),
        );
    }

    pub fn permit(&self, permit: &BrainRefreshPermit) {
        let mut fields =
            Map::from_iter([("checking_revision".into(), json!(permit.checking_revision))]);
        opaque_field(&mut fields, "run_id", &permit.run_id);
        // Permit evidence describes the admitted check, never a publication.
        self.emit(
            ObservationPhase::Permit,
            ObservationOutcome::Admitted,
            fields,
        );
    }

    pub fn publication(&self, phase: ObservationPhase, record: &Value) {
        self.emit(
            phase,
            ObservationOutcome::Published,
            publication_fields(record),
        );
    }
}

fn opaque_field(fields: &mut Map<String, Value>, name: &str, value: &str) {
    if value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        fields.insert(name.into(), json!(value));
    }
}

fn publication_fields(record: &Value) -> Map<String, Value> {
    let mut fields = Map::new();
    if let Some(revision) = record.get("revision").and_then(Value::as_u64) {
        fields.insert("revision".into(), json!(revision));
    }
    if let Some(marker) = record
        .pointer("/runtime_failure_marker/marker_id")
        .and_then(Value::as_str)
    {
        opaque_field(&mut fields, "runtime_failure_marker", marker);
    }
    let contract = &local_contract().brain_state;
    if let Some(reason) = record
        .get("reason_code")
        .and_then(Value::as_str)
        .filter(|reason| reason.len() <= 128 && contract.reason_codes.iter().any(|r| r == reason))
    {
        fields.insert("reason_code".into(), json!(reason));
        if let Some(reason_value) = record
            .pointer("/diagnostic/verification_reason")
            .and_then(Value::as_str)
            .filter(|value| value.len() <= 128)
            && contract
                .diagnostic_metadata_schemas
                .get(reason)
                .and_then(|schema| schema.get("verification_reason"))
                .is_some_and(|allowed| allowed.iter().any(|value| value == reason_value))
        {
            fields.insert("verification_reason".into(), json!(reason_value));
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_selects_only_bounded_contract_fields() {
        let mut record = json!({"revision": 7, "reason_code": "attestation_rejected",
            "diagnostic":{"verification_reason":"proof_http_eof","secret":"excluded"},
            "runtime_failure_marker":{"marker_id":"0123456789abcdef0123456789abcdef"},
            "prompt":"excluded","credential_holds":{"excluded":"excluded"}});
        let fields = publication_fields(&record);
        assert_eq!(fields, serde_json::from_value::<Map<String, Value>>(json!({
            "revision":7,"reason_code":"attestation_rejected","verification_reason":"proof_http_eof",
            "runtime_failure_marker":"0123456789abcdef0123456789abcdef"
        })).unwrap());
        record["reason_code"] = json!("arbitrary content");
        record["runtime_failure_marker"]["marker_id"] = json!("arbitrary content");
        assert_eq!(
            publication_fields(&record),
            Map::from_iter([("revision".into(), json!(7))])
        );
    }
}
