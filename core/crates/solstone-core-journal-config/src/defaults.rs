// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::path::{Path, PathBuf};

use nix::unistd::{Uid, User};
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
  "agent": {
    "name": "sol",
    "name_status": "default",
    "named_date": null
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
    "home_address": null
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
    let (name, preferred) = resolve_identity();
    let timezone = resolve_timezone();
    let identity = config
        .get_mut("identity")
        .and_then(Value::as_object_mut)
        .expect("journal config defaults must contain an identity object");
    identity.insert("name".to_owned(), Value::String(name));
    identity.insert("preferred".to_owned(), Value::String(preferred));
    identity.insert("timezone".to_owned(), Value::String(timezone));
    config
}

fn resolve_identity() -> (String, String) {
    let Ok(Some(user)) = current_user() else {
        return (String::new(), String::new());
    };
    let name = user
        .gecos
        .split_once(',')
        .map_or(user.gecos.as_str(), |(name, _)| name);
    (python_strip(name).to_owned(), user.login)
}

fn resolve_timezone() -> String {
    let Ok(path) = resolved_localtime() else {
        return String::new();
    };
    zone_from_localtime_path(&path.to_string_lossy())
}

fn zone_from_localtime_path(resolved: &str) -> String {
    let marker = "/zoneinfo/";
    resolved.rfind(marker).map_or_else(String::new, |index| {
        resolved[index + marker.len()..].to_owned()
    })
}

#[derive(Clone)]
pub(crate) struct PasswdRecord {
    pub(crate) gecos: String,
    pub(crate) login: String,
}

pub(crate) trait PasswdSource {
    fn current_user(&self) -> Result<Option<PasswdRecord>, Box<dyn Error + Send + Sync>>;
}

struct SystemPasswdSource;

impl PasswdSource for SystemPasswdSource {
    fn current_user(&self) -> Result<Option<PasswdRecord>, Box<dyn Error + Send + Sync>> {
        User::from_uid(Uid::current())
            .map(|user| {
                user.map(|user| PasswdRecord {
                    gecos: user.gecos.to_string_lossy().into_owned(),
                    login: user.name,
                })
            })
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)
    }
}

pub(crate) trait LocaltimeSource {
    fn resolved_localtime(&self) -> Result<PathBuf, Box<dyn Error + Send + Sync>>;
}

struct SystemLocaltimeSource;

impl LocaltimeSource for SystemLocaltimeSource {
    fn resolved_localtime(&self) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
        Path::new("/etc/localtime")
            .canonicalize()
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)
    }
}

fn current_user() -> Result<Option<PasswdRecord>, Box<dyn Error + Send + Sync>> {
    #[cfg(test)]
    if let Some(source) = test_passwd_source() {
        return source.current_user();
    }
    SystemPasswdSource.current_user()
}

fn resolved_localtime() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    #[cfg(test)]
    if let Some(source) = test_localtime_source() {
        return source.resolved_localtime();
    }
    SystemLocaltimeSource.resolved_localtime()
}

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::rc::Rc;

#[cfg(test)]
thread_local! {
    static TEST_PASSWD_SOURCE: RefCell<Option<Rc<dyn PasswdSource>>> = const { RefCell::new(None) };
    static TEST_LOCALTIME_SOURCE: RefCell<Option<Rc<dyn LocaltimeSource>>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn test_passwd_source() -> Option<Rc<dyn PasswdSource>> {
    TEST_PASSWD_SOURCE.with(|source| source.borrow().clone())
}

#[cfg(test)]
fn test_localtime_source() -> Option<Rc<dyn LocaltimeSource>> {
    TEST_LOCALTIME_SOURCE.with(|source| source.borrow().clone())
}

#[cfg(test)]
pub(crate) struct PasswdSourceGuard {
    previous: Option<Rc<dyn PasswdSource>>,
}

#[cfg(test)]
impl Drop for PasswdSourceGuard {
    fn drop(&mut self) {
        TEST_PASSWD_SOURCE.with(|source| {
            *source.borrow_mut() = self.previous.take();
        });
    }
}

#[cfg(test)]
pub(crate) struct LocaltimeSourceGuard {
    previous: Option<Rc<dyn LocaltimeSource>>,
}

#[cfg(test)]
impl Drop for LocaltimeSourceGuard {
    fn drop(&mut self) {
        TEST_LOCALTIME_SOURCE.with(|source| {
            *source.borrow_mut() = self.previous.take();
        });
    }
}

#[cfg(test)]
pub(crate) fn install_passwd_source(source: Rc<dyn PasswdSource>) -> PasswdSourceGuard {
    let previous = TEST_PASSWD_SOURCE.with(|current| current.replace(Some(source)));
    PasswdSourceGuard { previous }
}

#[cfg(test)]
pub(crate) fn install_localtime_source(source: Rc<dyn LocaltimeSource>) -> LocaltimeSourceGuard {
    let previous = TEST_LOCALTIME_SOURCE.with(|current| current.replace(Some(source)));
    LocaltimeSourceGuard { previous }
}
