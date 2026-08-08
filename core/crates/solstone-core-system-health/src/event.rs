// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct RunLogRecord {
    pub ts: i64,
    pub event: HealthEvent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HealthEvent {
    ActivityDetected(EventPayload),
    ActivityPersisted(EventPayload),
    ActivityPromptsSkipped(EventPayload),
    ActivityUnchanged(EventPayload),
    GroupStart(EventPayload),
    GroupComplete(EventPayload),
    MemoryThrottleComplete(EventPayload),
    PhaseStart(EventPayload),
    PhaseComplete(EventPayload),
    RunStart(EventPayload),
    RunComplete(EventPayload),
    SenseSkip(EventPayload),
    SenseComplete(EventPayload),
    SenseChangeDetect(EventPayload),
    TalentDispatch(EventPayload),
    TalentComplete(EventPayload),
    TalentFail(EventPayload),
    TalentSkip(EventPayload),
    Unknown(String, Map<String, Value>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventPayload {
    pub mode: Option<String>,
    pub day: Option<String>,
    pub segment: Option<String>,
    pub stream: Option<String>,
    pub facet: Option<String>,
    pub activity: Option<String>,
    pub name: Option<String>,
    pub use_id: Option<String>,
    pub reference: Option<String>,
    pub reason: Option<String>,
    pub detail: Option<String>,
    pub state: Option<String>,
    pub reason_code: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub density: Option<String>,
    pub change_class: Option<String>,
    pub phase: Option<String>,
    pub predecessor: Option<String>,
    pub duration_ms: Option<i64>,
    pub success: Option<i64>,
    pub failed: Option<i64>,
    pub skipped: Option<i64>,
    pub count: Option<i64>,
    pub priority: Option<i64>,
    pub timeout_seconds: Option<i64>,
    pub cleared: Option<i64>,
    pub remaining: Option<i64>,
    pub cache_hit: Option<bool>,
    pub gated: Option<bool>,
    pub bounded: Option<bool>,
    pub extensions: Map<String, Value>,
}

impl HealthEvent {
    pub fn kind(&self) -> &str {
        match self {
            Self::ActivityDetected(_) => "activity.detected",
            Self::ActivityPersisted(_) => "activity.persisted",
            Self::ActivityPromptsSkipped(_) => "activity.prompts_skipped",
            Self::ActivityUnchanged(_) => "activity.unchanged",
            Self::GroupStart(_) => "group.start",
            Self::GroupComplete(_) => "group.complete",
            Self::MemoryThrottleComplete(_) => "memory_throttle.complete",
            Self::PhaseStart(_) => "phase.start",
            Self::PhaseComplete(_) => "phase.complete",
            Self::RunStart(_) => "run.start",
            Self::RunComplete(_) => "run.complete",
            Self::SenseSkip(_) => "sense.skip",
            Self::SenseComplete(_) => "sense.complete",
            Self::SenseChangeDetect(_) => "sense.change_detect",
            Self::TalentDispatch(_) => "talent.dispatch",
            Self::TalentComplete(_) => "talent.complete",
            Self::TalentFail(_) => "talent.fail",
            Self::TalentSkip(_) => "talent.skip",
            Self::Unknown(kind, _) => kind,
        }
    }

    fn from_kind(kind: String, fields: Map<String, Value>) -> Self {
        let payload = || EventPayload::from_fields(fields.clone());
        match kind.as_str() {
            "activity.detected" => Self::ActivityDetected(payload()),
            "activity.persisted" => Self::ActivityPersisted(payload()),
            "activity.prompts_skipped" => Self::ActivityPromptsSkipped(payload()),
            "activity.unchanged" => Self::ActivityUnchanged(payload()),
            "group.start" => Self::GroupStart(payload()),
            "group.complete" => Self::GroupComplete(payload()),
            "memory_throttle.complete" => Self::MemoryThrottleComplete(payload()),
            "phase.start" => Self::PhaseStart(payload()),
            "phase.complete" => Self::PhaseComplete(payload()),
            "run.start" => Self::RunStart(payload()),
            "run.complete" => Self::RunComplete(payload()),
            "sense.skip" => Self::SenseSkip(payload()),
            "sense.complete" => Self::SenseComplete(payload()),
            "sense.change_detect" => Self::SenseChangeDetect(payload()),
            "talent.dispatch" => Self::TalentDispatch(payload()),
            "talent.complete" => Self::TalentComplete(payload()),
            "talent.fail" => Self::TalentFail(payload()),
            "talent.skip" => Self::TalentSkip(payload()),
            _ => Self::Unknown(kind, fields),
        }
    }

    fn fields(&self) -> Map<String, Value> {
        match self {
            Self::ActivityDetected(payload)
            | Self::ActivityPersisted(payload)
            | Self::ActivityPromptsSkipped(payload)
            | Self::ActivityUnchanged(payload)
            | Self::GroupStart(payload)
            | Self::GroupComplete(payload)
            | Self::MemoryThrottleComplete(payload)
            | Self::PhaseStart(payload)
            | Self::PhaseComplete(payload)
            | Self::RunStart(payload)
            | Self::RunComplete(payload)
            | Self::SenseSkip(payload)
            | Self::SenseComplete(payload)
            | Self::SenseChangeDetect(payload)
            | Self::TalentDispatch(payload)
            | Self::TalentComplete(payload)
            | Self::TalentFail(payload)
            | Self::TalentSkip(payload) => payload.to_fields(),
            Self::Unknown(_, fields) => fields.clone(),
        }
    }

    pub(crate) fn payload(&self) -> Option<&EventPayload> {
        match self {
            Self::ActivityDetected(payload)
            | Self::ActivityPersisted(payload)
            | Self::ActivityPromptsSkipped(payload)
            | Self::ActivityUnchanged(payload)
            | Self::GroupStart(payload)
            | Self::GroupComplete(payload)
            | Self::MemoryThrottleComplete(payload)
            | Self::PhaseStart(payload)
            | Self::PhaseComplete(payload)
            | Self::RunStart(payload)
            | Self::RunComplete(payload)
            | Self::SenseSkip(payload)
            | Self::SenseComplete(payload)
            | Self::SenseChangeDetect(payload)
            | Self::TalentDispatch(payload)
            | Self::TalentComplete(payload)
            | Self::TalentFail(payload)
            | Self::TalentSkip(payload) => Some(payload),
            Self::Unknown(_, _) => None,
        }
    }
}

impl EventPayload {
    fn from_fields(mut fields: Map<String, Value>) -> Self {
        macro_rules! take_string {
            ($name:ident, $key:literal) => {
                take_string(&mut fields, $key)
            };
        }
        macro_rules! take_i64 {
            ($name:ident, $key:literal) => {
                take_i64(&mut fields, $key)
            };
        }
        macro_rules! take_bool {
            ($name:ident, $key:literal) => {
                take_bool(&mut fields, $key)
            };
        }
        Self {
            mode: take_string!(mode, "mode"),
            day: take_string!(day, "day"),
            segment: take_string!(segment, "segment"),
            stream: take_string!(stream, "stream"),
            facet: take_string!(facet, "facet"),
            activity: take_string!(activity, "activity"),
            name: take_string!(name, "name"),
            use_id: take_string!(use_id, "use_id"),
            reference: take_string!(reference, "ref"),
            reason: take_string!(reason, "reason"),
            detail: take_string!(detail, "detail"),
            state: take_string!(state, "state"),
            reason_code: take_string!(reason_code, "reason_code"),
            provider: take_string!(provider, "provider"),
            model: take_string!(model, "model"),
            density: take_string!(density, "density"),
            change_class: take_string!(change_class, "change_class"),
            phase: take_string!(phase, "phase"),
            predecessor: take_string!(predecessor, "predecessor"),
            duration_ms: take_i64!(duration_ms, "duration_ms"),
            success: take_i64!(success, "success"),
            failed: take_i64!(failed, "failed"),
            skipped: take_i64!(skipped, "skipped"),
            count: take_i64!(count, "count"),
            priority: take_i64!(priority, "priority"),
            timeout_seconds: take_i64!(timeout_seconds, "timeout_seconds"),
            cleared: take_i64!(cleared, "cleared"),
            remaining: take_i64!(remaining, "remaining"),
            cache_hit: take_bool!(cache_hit, "cache_hit"),
            gated: take_bool!(gated, "gated"),
            bounded: take_bool!(bounded, "bounded"),
            extensions: fields,
        }
    }

    fn to_fields(&self) -> Map<String, Value> {
        let mut fields = self.extensions.clone();
        macro_rules! string {
            ($value:expr, $key:literal) => {
                if let Some(value) = &$value {
                    fields.insert($key.to_owned(), Value::String(value.clone()));
                }
            };
        }
        macro_rules! integer {
            ($value:expr, $key:literal) => {
                if let Some(value) = $value {
                    fields.insert($key.to_owned(), Value::from(value));
                }
            };
        }
        macro_rules! boolean {
            ($value:expr, $key:literal) => {
                if let Some(value) = $value {
                    fields.insert($key.to_owned(), Value::Bool(value));
                }
            };
        }
        string!(self.mode, "mode");
        string!(self.day, "day");
        string!(self.segment, "segment");
        string!(self.stream, "stream");
        string!(self.facet, "facet");
        string!(self.activity, "activity");
        string!(self.name, "name");
        string!(self.use_id, "use_id");
        string!(self.reference, "ref");
        string!(self.reason, "reason");
        string!(self.detail, "detail");
        string!(self.state, "state");
        string!(self.reason_code, "reason_code");
        string!(self.provider, "provider");
        string!(self.model, "model");
        string!(self.density, "density");
        string!(self.change_class, "change_class");
        string!(self.phase, "phase");
        string!(self.predecessor, "predecessor");
        integer!(self.duration_ms, "duration_ms");
        integer!(self.success, "success");
        integer!(self.failed, "failed");
        integer!(self.skipped, "skipped");
        integer!(self.count, "count");
        integer!(self.priority, "priority");
        integer!(self.timeout_seconds, "timeout_seconds");
        integer!(self.cleared, "cleared");
        integer!(self.remaining, "remaining");
        boolean!(self.cache_hit, "cache_hit");
        boolean!(self.gated, "gated");
        boolean!(self.bounded, "bounded");
        fields
    }
}

fn take_string(fields: &mut Map<String, Value>, key: &str) -> Option<String> {
    let value = fields.get(key)?;
    let string = value.as_str()?.to_owned();
    fields.remove(key);
    Some(string)
}
fn take_i64(fields: &mut Map<String, Value>, key: &str) -> Option<i64> {
    let value = fields.get(key)?;
    let integer = value.as_i64()?;
    fields.remove(key);
    Some(integer)
}
fn take_bool(fields: &mut Map<String, Value>, key: &str) -> Option<bool> {
    let value = fields.get(key)?;
    let boolean = value.as_bool()?;
    fields.remove(key);
    Some(boolean)
}

impl<'de> Deserialize<'de> for RunLogRecord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| D::Error::custom("record must be an object"))?;
        let event = object
            .remove("event")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::custom("event must be a string"))?;
        let ts = object
            .remove("ts")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| D::Error::custom("ts must be an i64 JSON integer"))?;
        Ok(Self {
            ts,
            event: HealthEvent::from_kind(event, object),
        })
    }
}

impl Serialize for RunLogRecord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut object = self.event.fields();
        object.insert(
            "event".to_owned(),
            Value::String(self.event.kind().to_owned()),
        );
        object.insert("ts".to_owned(), Value::from(self.ts));
        Value::Object(object).serialize(serializer)
    }
}
