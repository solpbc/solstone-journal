// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Write-owning SPL enrollment and posture operations.

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_journal_config::{get_journal_config_path, read_journal_config};
use solstone_core_journal_config_write::{
    ConfigMutationError, JournalConfigMutation, mutate_journal_config,
};
use solstone_core_journal_io::{AtomicWriteError, JsonWriteOptions, write_json};
use solstone_core_sol_link::service_identity::{
    ServiceIdentity, ServiceIdentityError, load_or_create_service_identity,
    load_service_identity_ca,
};
use thiserror::Error;

pub const DEFAULT_RELAY_URL: &str = "https://link.solstone.app";

#[derive(Debug, Error)]
pub enum EnrollError {
    #[error("relay rejected enrollment (HTTP {status})")]
    Rejected { status: u16, reason: Option<String> },
    #[error("relay is unreachable: {0}")]
    Unreachable(String),
    #[error("relay response was invalid: {0}")]
    Response(String),
}

#[derive(Debug, Error)]
pub enum EnableSplError {
    #[error("journal config is not initialized")]
    JournalNotInitialized,
    #[error(transparent)]
    Identity(#[from] ServiceIdentityError),
    #[error(transparent)]
    Enroll(#[from] EnrollError),
    #[error(transparent)]
    Mutation(#[from] ConfigMutationError),
    #[error("private-link posture was saved, but the service token was not: {0}")]
    Token(AtomicWriteError),
}

#[derive(Debug, Error)]
pub enum DisableSplError {
    #[error("journal config is not initialized")]
    JournalNotInitialized,
    #[error(transparent)]
    Mutation(#[from] ConfigMutationError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SplDisableOutcome {
    pub was_enabled: bool,
}

pub fn relay_url(journal_root: &Path) -> String {
    if let Ok(value) = std::env::var("SOL_LINK_RELAY_URL") {
        let value = value.trim();
        if !value.is_empty() {
            return value.trim_end_matches('/').to_owned();
        }
    }
    if let Ok(read) = read_journal_config(journal_root)
        && let Some(value) = read
            .config
            .as_ref()
            .and_then(|config| config.get("link"))
            .and_then(Value::as_object)
            .and_then(|link| link.get("relay_url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        return value.trim_end_matches('/').to_owned();
    }
    DEFAULT_RELAY_URL.to_owned()
}

pub fn enroll_home(
    relay_base_url: &str,
    instance_id: &str,
    ca_pubkey: &str,
    home_label: &str,
) -> Result<String, EnrollError> {
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .new_agent();
    let payload = serde_json::to_string(
        &json!({"instance_id": instance_id, "ca_pubkey": ca_pubkey, "home_label": home_label}),
    )
    .expect("enrollment payload serializes");
    let response = agent
        .post(&format!(
            "{}/enroll/home",
            relay_base_url.trim_end_matches('/')
        ))
        .header("Content-Type", "application/json")
        .send(payload);
    let response = match response {
        Ok(response) => response,
        Err(error) => return Err(EnrollError::Unreachable(error.to_string())),
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.into_body().read_to_string().unwrap_or_default();
        return map_enroll_response(status, &body);
    }
    let body = response.into_body().read_to_string().map_err(|error| {
        if matches!(error, ureq::Error::Timeout(_)) {
            EnrollError::Unreachable(error.to_string())
        } else {
            EnrollError::Response(error.to_string())
        }
    })?;
    map_enroll_response(status, &body)
}

fn map_enroll_response(status: u16, body: &str) -> Result<String, EnrollError> {
    if !(200..300).contains(&status) {
        let reason = serde_json::from_str::<Value>(body).ok().and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        return Err(EnrollError::Rejected { status, reason });
    }
    let value: Value = serde_json::from_str(body)
        .map_err(|_| EnrollError::Response("body was not JSON".to_owned()))?;
    value
        .get("service_token")
        .or_else(|| value.get("account_token"))
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| EnrollError::Response("service token was missing".to_owned()))
}

pub fn enable_spl(journal_root: &Path) -> Result<(), EnableSplError> {
    enable_spl_with(journal_root, |identity, ca_pubkey| {
        enroll_home(
            &relay_url(journal_root),
            &identity.instance_id,
            ca_pubkey,
            &identity.home_label,
        )
    })
}

pub fn enable_spl_with(
    journal_root: &Path,
    enroll: impl FnOnce(&ServiceIdentity, &str) -> Result<String, EnrollError>,
) -> Result<(), EnableSplError> {
    require_journal_config(journal_root)?;
    let identity = load_or_create_service_identity(journal_root, "solstone")?;
    let ca = load_service_identity_ca(journal_root)?;
    let token = enroll(&identity, &ca.public_key_spki_pem())?;
    write_posture(journal_root, "spl")?;
    save_service_token(journal_root, &token).map_err(EnableSplError::Token)
}

pub fn save_service_token(journal_root: &Path, token: &str) -> Result<(), AtomicWriteError> {
    let path = journal_root
        .join("link")
        .join("tokens")
        .join("account.json");
    std::fs::create_dir_all(path.parent().expect("token path parent exists")).map_err(
        |source| AtomicWriteError::Io {
            path: path.clone(),
            source,
        },
    )?;
    write_json(
        path,
        &json!({"service_token": token}),
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
}

pub fn disable_spl(journal_root: &Path) -> Result<SplDisableOutcome, DisableSplError> {
    require_journal_config(journal_root).map_err(|_| DisableSplError::JournalNotInitialized)?;
    let result = mutate_journal_config(journal_root, Default::default(), |config| {
        let enabled = config
            .get("link")
            .and_then(Value::as_object)
            .and_then(|link| link.get("posture"))
            .and_then(Value::as_str)
            == Some("spl");
        if enabled {
            object_at(config, "link")
                .insert("posture".to_owned(), Value::String("direct".to_owned()));
        }
        JournalConfigMutation {
            changed: enabled,
            value: SplDisableOutcome {
                was_enabled: enabled,
            },
        }
    })?;
    Ok(result.value)
}

fn require_journal_config(journal_root: &Path) -> Result<(), EnableSplError> {
    get_journal_config_path(journal_root)
        .is_file()
        .then_some(())
        .ok_or(EnableSplError::JournalNotInitialized)
}

fn write_posture(journal_root: &Path, posture: &str) -> Result<(), ConfigMutationError> {
    mutate_journal_config(journal_root, Default::default(), |config| {
        let changed = config
            .get("link")
            .and_then(Value::as_object)
            .and_then(|link| link.get("posture"))
            .and_then(Value::as_str)
            != Some(posture);
        if changed {
            object_at(config, "link")
                .insert("posture".to_owned(), Value::String(posture.to_owned()));
        }
        JournalConfigMutation { changed, value: () }
    })
    .map(|_| ())
}

fn object_at<'a>(parent: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !parent.get(key).is_some_and(Value::is_object) {
        parent.insert(key.to_owned(), Value::Object(Map::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object inserted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct TempJournal(PathBuf);
    impl TempJournal {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-private-link-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::create_dir_all(path.join("config")).unwrap();
            fs::write(
                path.join("config/journal.json"),
                b"{\"link\":{\"posture\":\"direct\"}}\n",
            )
            .unwrap();
            Self(path)
        }
    }
    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn map_enroll_response_accepts_a_legacy_account_token() {
        let result = map_enroll_response(200, r#"{"account_token":"legacy-token"}"#);
        assert_eq!(result.unwrap(), "legacy-token");
    }

    #[test]
    fn map_enroll_response_rejects_non_json_http_error_bodies() {
        let result = map_enroll_response(503, "temporarily unavailable");
        assert!(matches!(
            result,
            Err(EnrollError::Rejected {
                status: 503,
                reason: None,
            })
        ));
    }

    #[test]
    fn token_overwrite_drops_legacy_key() {
        let journal = TempJournal::new();
        let path = journal.0.join("link/tokens/account.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"account_token\":\"old\"}").unwrap();
        save_service_token(&journal.0, "new").unwrap();
        assert_eq!(
            fs::read_to_string(path)
                .unwrap()
                .replace(char::is_whitespace, ""),
            "{\"service_token\":\"new\"}"
        );
    }

    #[test]
    fn disable_noop_does_not_replace_config() {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        let journal = TempJournal::new();
        let path = journal.0.join("config/journal.json");
        let before = fs::read(&path).unwrap();
        #[cfg(unix)]
        let inode = fs::metadata(&path).unwrap().ino();
        assert_eq!(
            disable_spl(&journal.0).unwrap(),
            SplDisableOutcome { was_enabled: false }
        );
        assert_eq!(fs::read(path).unwrap(), before);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(journal.0.join("config/journal.json"))
                .unwrap()
                .ino(),
            inode
        );
    }

    #[test]
    fn disable_changes_only_spl_posture() {
        let journal = TempJournal::new();
        fs::write(
            journal.0.join("config/journal.json"),
            b"{\"link\":{\"posture\":\"spl\"}}\n",
        )
        .unwrap();
        assert_eq!(
            disable_spl(&journal.0).unwrap(),
            SplDisableOutcome { was_enabled: true }
        );
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(journal.0.join("config/journal.json")).unwrap()
            )
            .unwrap()["link"]["posture"],
            "direct"
        );
    }

    #[test]
    fn posture_is_saved_before_a_failed_token_write() {
        let journal = TempJournal::new();
        fs::create_dir_all(journal.0.join("link")).unwrap();
        fs::write(journal.0.join("link/tokens"), b"not a directory").unwrap();
        let error = enable_spl_with(&journal.0, |_, _| Ok("token".to_owned())).unwrap_err();
        assert!(matches!(error, EnableSplError::Token(_)));
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(journal.0.join("config/journal.json")).unwrap()
            )
            .unwrap()["link"]["posture"],
            "spl"
        );
    }
}
