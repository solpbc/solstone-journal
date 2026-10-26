// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Write-owning SPL enrollment and posture operations.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::service_token_write::{
    ServiceTokenPublication, ServiceTokenWriteRefusal, save_service_token_at_root_unlocked,
};
use serde_json::{Map, Value, json};
use solstone_core_journal_config::{get_journal_config_path, read_journal_config};
use solstone_core_journal_config_write::{
    BoundConfigOutcome, BoundConfigRefusal, ConfigMutationError, JournalConfigMutation,
    mutate_journal_config, mutate_journal_config_at_root_checked,
};
use solstone_core_journal_io::JournalRoot;
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
    #[error("private-link enable was interrupted")]
    Interrupted,
    #[error("journal config is not initialized")]
    JournalNotInitialized,
    #[error(transparent)]
    Identity(#[from] ServiceIdentityError),
    #[error(transparent)]
    Enroll(#[from] EnrollError),
    #[error(transparent)]
    Mutation(#[from] BoundConfigRefusal),
    #[error("private-link service token publication was refused")]
    Token(Box<SplTokenFailure>),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnrollMode {
    Establish,
    Renew,
}

pub fn enroll_home(
    relay_base_url: &str,
    instance_id: &str,
    ca_pubkey: &str,
    _home_label: &str,
    mode: EnrollMode,
) -> Result<String, EnrollError> {
    let agent = crate::relay_access::build_isolated_ureq_agent(Duration::from_secs(30));
    let payload = match mode {
        EnrollMode::Establish => {
            serde_json::to_string(&json!({"instance_id": instance_id, "ca_pubkey": ca_pubkey}))
                .expect("enrollment payload serializes")
        }
        EnrollMode::Renew => serde_json::to_string(
            &json!({"instance_id": instance_id, "ca_pubkey": ca_pubkey, "mode": "renew"}),
        )
        .expect("enrollment payload serializes"),
    };
    let response = agent
        .post(&format!(
            "{}/enroll/home",
            relay_base_url.trim_end_matches('/')
        ))
        .header("Content-Type", "application/json")
        .header("User-Agent", "")
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

/// Publication facts for the two independently atomic local changes.
#[derive(Debug)]
pub struct SplEnablePublication {
    pub posture: BoundConfigOutcome,
    /// Absent when the posture landed but its confirmation was uncertain.
    pub token: Option<ServiceTokenPublication>,
}

impl SplEnablePublication {
    pub fn confirmed(&self) -> bool {
        config_confirmed(&self.posture)
            && self
                .token
                .as_ref()
                .is_some_and(ServiceTokenPublication::confirmed)
    }
}

#[derive(Debug)]
pub struct SplTokenFailure {
    pub posture: BoundConfigOutcome,
    pub refusal: ServiceTokenWriteRefusal,
}

fn config_confirmed(outcome: &BoundConfigOutcome) -> bool {
    match outcome {
        BoundConfigOutcome::Unchanged => true,
        BoundConfigOutcome::Published(p) => {
            p.durability_uncertainty.is_none()
                && p.observation_uncertainty.is_none()
                && p.confirmation_refusal.is_none()
        }
    }
}

pub fn enable_spl(journal_root: &Path) -> Result<SplEnablePublication, EnableSplError> {
    enable_spl_with(
        journal_root,
        |identity, ca_pubkey| {
            enroll_home(
                &relay_url(journal_root),
                &identity.instance_id,
                ca_pubkey,
                &identity.home_label,
                EnrollMode::Establish,
            )
        },
        || true,
    )
}

/// One-shot desktop entry: existing identity establishment precedes enrollment.
/// Embedded consent passes its already prepared identity to the bound entry.
pub fn enable_spl_with(
    journal_root: &Path,
    enroll: impl FnOnce(&ServiceIdentity, &str) -> Result<String, EnrollError>,
    active: impl Fn() -> bool,
) -> Result<SplEnablePublication, EnableSplError> {
    if !active() {
        return Err(EnableSplError::Interrupted);
    }
    require_journal_config(journal_root)?;
    let root = Arc::new(
        JournalRoot::open(journal_root)
            .map_err(|e| EnableSplError::Mutation(BoundConfigRefusal::Admission(Box::new(e))))?,
    );
    let identity = load_or_create_service_identity(journal_root)?;
    let ca = load_service_identity_ca(journal_root)?;
    enable_spl_at_root_with(&root, &identity, &ca.public_key_spki_pem(), enroll, active)
}

/// Publish only through the originally admitted root. Identity and CA belong to
/// the already signed consent request; this entry never establishes identity.
pub fn enable_spl_at_root_with(
    root: &Arc<JournalRoot>,
    identity: &ServiceIdentity,
    ca_pubkey: &str,
    enroll: impl FnOnce(&ServiceIdentity, &str) -> Result<String, EnrollError>,
    active: impl Fn() -> bool,
) -> Result<SplEnablePublication, EnableSplError> {
    let check = || {
        if !active() {
            return Err("private-link enable was interrupted".to_owned());
        }
        root.revalidate_canonical_binding()
            .map_err(|e| e.to_string())
    };
    check().map_err(|_| EnableSplError::Interrupted)?;
    let token = enroll(identity, ca_pubkey)?;
    crate::relay_access::mutate_service_configuration(root.canonical_path(), || {
        check().map_err(|_| EnableSplError::Interrupted)?;
        let posture =
            mutate_journal_config_at_root_checked(root, Default::default(), &check, |config| {
                let changed = config
                    .get("link")
                    .and_then(Value::as_object)
                    .and_then(|link| link.get("posture"))
                    .and_then(Value::as_str)
                    != Some("spl");
                if changed {
                    object_at(config, "link")
                        .insert("posture".to_owned(), Value::String("spl".to_owned()));
                }
                JournalConfigMutation { changed, value: () }
            })?
            .outcome;
        if !config_confirmed(&posture) {
            return Ok(SplEnablePublication {
                posture,
                token: None,
            });
        }
        match save_service_token_at_root_unlocked(root, &token, &check) {
            Ok(token) => Ok(SplEnablePublication {
                posture,
                token: Some(token),
            }),
            Err(refusal) => Err(EnableSplError::Token(Box::new(SplTokenFailure {
                posture,
                refusal,
            }))),
        }
    })
}

/// One-shot caller: capture a root once and retain actual publication facts.
pub fn save_service_token(
    journal_root: &Path,
    token: &str,
) -> Result<ServiceTokenPublication, ServiceTokenWriteRefusal> {
    let root =
        Arc::new(JournalRoot::open(journal_root).map_err(ServiceTokenWriteRefusal::from_error)?);
    crate::relay_access::mutate_service_configuration(root.canonical_path(), || {
        save_service_token_at_root_unlocked(&root, token, &|| Ok(()))
    })
}

pub fn disable_spl(journal_root: &Path) -> Result<SplDisableOutcome, DisableSplError> {
    require_journal_config(journal_root).map_err(|_| DisableSplError::JournalNotInitialized)?;
    crate::relay_access::mutate_service_configuration(journal_root, || {
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
    })
}

fn require_journal_config(journal_root: &Path) -> Result<(), EnableSplError> {
    get_journal_config_path(journal_root)
        .is_file()
        .then_some(())
        .ok_or(EnableSplError::JournalNotInitialized)
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
    fn interrupted_enrollment_cannot_publish_posture_or_token() {
        let journal = TempJournal::new();
        let config = journal.0.join("config/journal.json");
        let before = fs::read(&config).unwrap();
        let active = std::cell::Cell::new(true);
        let result = enable_spl_with(
            &journal.0,
            |_, _| {
                active.set(false);
                Ok("synthetic-token".to_owned())
            },
            || active.get(),
        );
        assert!(matches!(result, Err(EnableSplError::Interrupted)));
        assert_eq!(fs::read(config).unwrap(), before);
        assert!(!journal.0.join("link/tokens/account.json").exists());
    }

    #[test]
    fn posture_is_saved_before_a_failed_token_write() {
        let journal = TempJournal::new();
        fs::create_dir_all(journal.0.join("link")).unwrap();
        fs::write(journal.0.join("link/tokens"), b"not a directory").unwrap();
        let error =
            enable_spl_with(&journal.0, |_, _| Ok("token".to_owned()), || true).unwrap_err();
        assert!(matches!(error, EnableSplError::Token(_)));
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(journal.0.join("config/journal.json")).unwrap()
            )
            .unwrap()["link"]["posture"],
            "spl"
        );
    }

    #[cfg(all(unix, feature = "test-hooks"))]
    #[test]
    fn bound_token_publication_never_reopens_a_substituted_root_or_parent() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive as Primitive, run_with_bound_publication_barrier,
        };
        for replace_root in [true, false] {
            for primitive in [Primitive::TempCreate, Primitive::AuthorizedRename] {
                let journal = TempJournal::new();
                let root = Arc::new(JournalRoot::open(&journal.0).unwrap());
                let save = |token| save_service_token_at_root_unlocked(&root, token, &|| Ok(()));
                assert!(save("original-token").unwrap().confirmed());
                let target = if replace_root {
                    journal.0.clone()
                } else {
                    journal.0.join("link/tokens")
                };
                let retained = target.with_extension("retained");
                let replacement_token = if replace_root {
                    target.join("link/tokens/account.json")
                } else {
                    target.join("account.json")
                };
                let retained_token = if replace_root {
                    retained.join("link/tokens/account.json")
                } else {
                    retained.join("account.json")
                };
                let moved = retained.clone();
                let replaced = target.clone();
                let control = replacement_token.clone();
                let (result, fired) = run_with_bound_publication_barrier(
                    primitive,
                    1,
                    move || {
                        fs::rename(&replaced, &moved).unwrap();
                        fs::create_dir_all(control.parent().unwrap()).unwrap();
                        fs::write(control, b"replacement-control").unwrap();
                    },
                    || save("new-synthetic-secret"),
                );
                assert!(fired);
                assert_eq!(
                    fs::read(&replacement_token).unwrap(),
                    b"replacement-control"
                );
                if primitive == Primitive::AuthorizedRename {
                    let publication = result.unwrap();
                    assert!(!publication.confirmed());
                    assert!(publication.confirmation_refusal.is_some());
                    assert!(matches!(
                        publication.native(),
                        solstone_core_journal_io::BoundAtomicOutcome::Published { .. }
                    ));
                    assert!(
                        String::from_utf8(fs::read(&retained_token).unwrap())
                            .unwrap()
                            .contains("new-synthetic-secret")
                    );
                    assert!(!format!("{publication:?}").contains("new-synthetic-secret"));
                } else {
                    let error = result.unwrap_err();
                    assert!(!format!("{error:?} {error}").contains("new-synthetic-secret"));
                    assert!(
                        String::from_utf8(fs::read(&retained_token).unwrap())
                            .unwrap()
                            .contains("original-token")
                    );
                }
                fs::remove_dir_all(retained).unwrap();
            }
        }
    }

    #[cfg(all(unix, feature = "test-hooks"))]
    #[test]
    fn bound_token_native_uncertainty_and_late_cancel_remain_publications() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive as Primitive, run_with_bound_publication_barrier,
            run_with_bound_publication_fault,
        };
        let journal = TempJournal::new();
        let root = Arc::new(JournalRoot::open(&journal.0).unwrap());
        {
            let primitive = Primitive::ParentSync;
            let (result, consumed) = run_with_bound_publication_fault(primitive, 1, 5, || {
                save_service_token_at_root_unlocked(&root, "uncertain-secret", &|| Ok(()))
            });
            assert!(consumed);
            let publication = result.unwrap();
            assert!(!publication.confirmed());
            assert!(
                String::from_utf8(fs::read(journal.0.join("link/tokens/account.json")).unwrap())
                    .unwrap()
                    .contains("uncertain-secret")
            );
            assert!(!format!("{publication:?}").contains("uncertain-secret"));
        }
        let published_path = journal.0.join("link/tokens/account.json");
        let (result, fired) = run_with_bound_publication_barrier(
            Primitive::ParentSync,
            1,
            move || fs::remove_file(&published_path).unwrap(),
            || save_service_token_at_root_unlocked(&root, "observation-secret", &|| Ok(())),
        );
        assert!(fired);
        let publication = result.unwrap();
        assert!(!publication.confirmed());
        assert!(matches!(
            publication.native(),
            solstone_core_journal_io::BoundAtomicOutcome::PublishedObservationUncertain { .. }
        ));
        assert!(publication.confirmation_refusal.is_some());
        assert!(!format!("{publication:?}").contains("observation-secret"));
        let active = std::rc::Rc::new(std::cell::Cell::new(true));
        let cancelled = active.clone();
        let (result, fired) = run_with_bound_publication_barrier(
            Primitive::AuthorizedRename,
            1,
            move || cancelled.set(false),
            || {
                save_service_token_at_root_unlocked(&root, "late-secret", &|| {
                    if active.get() {
                        Ok(())
                    } else {
                        Err("cancelled".to_owned())
                    }
                })
            },
        );
        assert!(fired);
        let publication = result.unwrap();
        assert!(!publication.confirmed());
        assert!(publication.confirmation_refusal.is_some());
        let before = fs::read(journal.0.join("link/tokens/account.json")).unwrap();
        assert!(
            save_service_token_at_root_unlocked(&root, "withheld", &|| Err("cancelled".to_owned()))
                .is_err()
        );
        assert_eq!(
            fs::read(journal.0.join("link/tokens/account.json")).unwrap(),
            before
        );
    }

    #[cfg(all(unix, feature = "test-hooks"))]
    #[test]
    fn enable_preserves_posture_when_confirmation_or_the_later_token_refuses() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };
        let journal = TempJournal::new();
        let identity = load_or_create_service_identity(&journal.0).unwrap();
        let ca = load_service_identity_ca(&journal.0).unwrap();
        let root = Arc::new(JournalRoot::open(&journal.0).unwrap());
        let enrollments = std::cell::Cell::new(0);
        let enroll = |_: &ServiceIdentity, _: &str| {
            enrollments.set(enrollments.get() + 1);
            Ok("aggregate-secret".to_owned())
        };
        let (result, consumed) =
            run_with_bound_publication_fault(BoundPublicationPrimitive::ParentSync, 1, 5, || {
                enable_spl_at_root_with(&root, &identity, &ca.public_key_spki_pem(), enroll, || {
                    true
                })
            });
        assert!(consumed);
        let publication = result.unwrap();
        assert!(matches!(
            publication.posture,
            BoundConfigOutcome::Published(_)
        ));
        assert!(!publication.confirmed());
        assert!(publication.token.is_none());
        assert!(!journal.0.join("link/tokens").exists());
        assert_eq!(enrollments.get(), 1);
        assert!(!format!("{publication:?}").contains("aggregate-secret"));

        disable_spl(&journal.0).unwrap();
        fs::write(journal.0.join("link/tokens"), b"blocked").unwrap();
        let error =
            enable_spl_at_root_with(&root, &identity, &ca.public_key_spki_pem(), enroll, || true)
                .unwrap_err();
        let EnableSplError::Token(ref failure) = error else {
            panic!("token must refuse after posture publication");
        };
        assert!(matches!(failure.posture, BoundConfigOutcome::Published(_)));
        assert!(config_confirmed(&failure.posture));
        assert_eq!(enrollments.get(), 2);
        assert_eq!(fs::read(journal.0.join("link/tokens")).unwrap(), b"blocked");
        assert!(!format!("{error:?} {error}").contains("aggregate-secret"));
        let config: Value =
            serde_json::from_slice(&fs::read(journal.0.join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(config["link"]["posture"], "spl");
        let before = fs::read(journal.0.join("config/journal.json")).unwrap();
        let error =
            enable_spl_at_root_with(&root, &identity, &ca.public_key_spki_pem(), enroll, || true)
                .unwrap_err();
        let EnableSplError::Token(failure) = error else {
            panic!("token remains refused");
        };
        assert!(matches!(failure.posture, BoundConfigOutcome::Unchanged));
        assert_eq!(
            fs::read(journal.0.join("config/journal.json")).unwrap(),
            before
        );
        assert_eq!(enrollments.get(), 3);
    }
}
