// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_journal::python_strip;

const DEFAULT_CONFIG_JSON: &str = r#"
{
  "identity": {
    "name": "",
    "preferred": "",
    "bio": "",
    "pronouns": {
      "subject": "",
      "object": "",
      "possessive": "",
      "reflexive": ""
    },
    "aliases": [],
    "email_addresses": [],
    "timezone": ""
  },
  "support": {
    "enabled": true,
    "proactive": true,
    "anonymous_feedback": false,
    "portal_url": "https://support.solstone.app"
  },
  "describe": {
    "max_concurrent": 1,
    "redact": [
      "use *** instead of any visible passwords, credentials, keys, tokens, and secrets",
      "completely omit and ignore any NSFW or adult content, do not mention or note it",
      "use *** instead of any visible credit card numbers, bank account numbers, and government ID numbers"
    ]
  },
  "transcribe": {
    "max_concurrent": 1
  },
  "voice": {
    "openai_api_key": null,
    "model": "gpt-realtime",
    "brain_model": "haiku"
  },
  "processing": {
    "mode": "realtime",
    "gate": {
      "time_window": { "enabled": true, "start": "02:00", "end": "06:00" },
      "display_powersave": { "enabled": false }
    }
  },
  "pairing": {
    "home_address": null,
    "direct_port": 7657
  },
  "backup": {
    "enabled": false,
    "mode": "byo",
    "destination": {
      "repository": null,
      "backend": null,
      "credentials": {}
    },
    "daily_key": null,
    "recovery_key": null,
    "confirmed_recovery_key": false,
    "retention": {
      "hourly": 24,
      "daily": 7,
      "weekly": 4,
      "monthly": 12
    },
    "schedule": {
      "every": "daily",
      "enabled": false
    },
    "last_backup": {
      "time": null,
      "snapshot_id": null,
      "status": null,
      "error_reason": null
    }
  },
  "retention": {
    "raw_media": "keep",
    "raw_media_days": null,
    "empty_audio": "processed",
    "empty_audio_days": null,
    "journal_logs": {
      "enabled": true,
      "days": 30
    },
    "per_stream": {},
    "storage_warning_disk_percent": 80,
    "storage_warning_raw_media_gb": null
  }
}
"#;

/// Return the canonical unmaterialized journal defaults.
pub fn plain_defaults() -> Map<String, Value> {
    serde_json::from_str::<Value>(DEFAULT_CONFIG_JSON)
        .expect("journal config defaults must remain valid JSON")
        .as_object()
        .cloned()
        .expect("journal config defaults must remain a JSON object")
}

/// Return canonical defaults with identity values resolved from the operating system.
pub fn materialized_defaults() -> Map<String, Value> {
    let mut config = plain_defaults();
    let (name, preferred, timezone) = resolved_identity();
    let identity = config
        .get_mut("identity")
        .and_then(Value::as_object_mut)
        .expect("journal config defaults must contain an identity object");
    identity.insert("name".to_owned(), Value::String(name));
    identity.insert("preferred".to_owned(), Value::String(preferred));
    identity.insert("timezone".to_owned(), Value::String(timezone));
    config
}

#[derive(Debug)]
pub(crate) struct IdentityError(String);

impl std::fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for IdentityError {}

impl From<whoami::Error> for IdentityError {
    fn from(error: whoami::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<iana_time_zone::GetTimezoneError> for IdentityError {
    fn from(error: iana_time_zone::GetTimezoneError) -> Self {
        Self(error.to_string())
    }
}

impl From<&str> for IdentityError {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for IdentityError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

pub(crate) trait IdentitySource {
    fn real_name(&self) -> Result<String, IdentityError>;
    fn user_name(&self) -> Result<String, IdentityError>;
    fn timezone(&self) -> Result<String, IdentityError>;
}

fn accept(value: Result<String, IdentityError>) -> Option<String> {
    let Ok(value) = value else {
        return None;
    };
    let stripped = python_strip(&value);
    if stripped.chars().any(char::is_control) || stripped.is_empty() {
        return None;
    }
    Some(stripped.to_owned())
}

pub(crate) fn fallback_identity(
    real_name: Result<String, IdentityError>,
    user_name: Result<String, IdentityError>,
    timezone: Result<String, IdentityError>,
) -> (String, String, String) {
    let name_c = accept(real_name);
    let user_c = accept(user_name);
    let timezone = accept(timezone).unwrap_or_default();
    let preferred = user_c.clone().unwrap_or_default();
    let name = name_c.or(user_c).unwrap_or_default();
    (name, preferred, timezone)
}

struct SystemIdentitySource;

impl IdentitySource for SystemIdentitySource {
    fn real_name(&self) -> Result<String, IdentityError> {
        whoami::realname().map_err(IdentityError::from)
    }

    fn user_name(&self) -> Result<String, IdentityError> {
        whoami::username().map_err(IdentityError::from)
    }

    fn timezone(&self) -> Result<String, IdentityError> {
        iana_time_zone::get_timezone().map_err(IdentityError::from)
    }
}

fn resolved_identity() -> (String, String, String) {
    #[cfg(test)]
    if let Some(source) = test_identity_source() {
        return fallback_identity(source.real_name(), source.user_name(), source.timezone());
    }
    let source = SystemIdentitySource;
    fallback_identity(source.real_name(), source.user_name(), source.timezone())
}

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::rc::Rc;

#[cfg(test)]
thread_local! {
    static TEST_IDENTITY_SOURCE: RefCell<Option<Rc<dyn IdentitySource>>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn test_identity_source() -> Option<Rc<dyn IdentitySource>> {
    TEST_IDENTITY_SOURCE.with(|source| source.borrow().clone())
}

#[cfg(test)]
pub(crate) struct IdentitySourceGuard {
    previous: Option<Rc<dyn IdentitySource>>,
}

#[cfg(test)]
impl Drop for IdentitySourceGuard {
    fn drop(&mut self) {
        TEST_IDENTITY_SOURCE.with(|source| {
            *source.borrow_mut() = self.previous.take();
        });
    }
}

#[cfg(test)]
pub(crate) fn install_identity_source(source: Rc<dyn IdentitySource>) -> IdentitySourceGuard {
    let previous = TEST_IDENTITY_SOURCE.with(|current| current.replace(Some(source)));
    IdentitySourceGuard { previous }
}
