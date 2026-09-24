// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Web Push VAPID key loading, creation, and RFC 8292 JWT signing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};

const VAPID_FILE: &str = "push-vapid.json";
const OWNER_ONLY_MODE: u32 = 0o600;

#[cfg(test)]
static TEST_ABSENT_HOOK: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_test_absent_hook(hook: Option<Box<dyn Fn() + Send + Sync>>) {
    *TEST_ABSENT_HOOK.lock().unwrap() = hook;
}

#[cfg(test)]
pub(crate) fn trigger_test_absent_hook() {
    if let Some(hook) = TEST_ABSENT_HOOK.lock().unwrap().as_ref() {
        hook();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct VapidDocument {
    pkcs8: String,
    created_at: String,
}

pub(crate) struct VapidKey {
    key_pair: EcdsaKeyPair,
    public_key_uncompressed: [u8; 65],
}

impl VapidKey {
    #[cfg(test)]
    pub(crate) fn public_key_uncompressed(&self) -> &[u8; 65] {
        &self.public_key_uncompressed
    }

    pub(crate) fn public_key_base64url(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.public_key_uncompressed)
    }

    pub(crate) fn sign_jwt(&self, origin: &str, now_unix: i64) -> Result<String, VapidError> {
        let header_json = r#"{"typ":"JWT","alg":"ES256"}"#;
        let exp = now_unix + 43200;
        let claims_json = format!(r#"{{"aud":"{origin}","exp":{exp}}}"#);

        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());
        let signing_input = format!("{header_b64}.{claims_b64}");

        let rng = SystemRandom::new();
        let sig = self
            .key_pair
            .sign(&rng, signing_input.as_bytes())
            .map_err(|_| VapidError::Sign)?;

        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.as_ref());
        Ok(format!("{signing_input}.{sig_b64}"))
    }
}

#[derive(Debug)]
pub(crate) enum VapidLoadError {
    NotFound,
    Unavailable { path: PathBuf, detail: String },
}

impl std::fmt::Display for VapidLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "vapid key not found"),
            Self::Unavailable { path, detail } => {
                write!(f, "vapid key unavailable at {}: {detail}", path.display())
            }
        }
    }
}

impl std::error::Error for VapidLoadError {}

#[derive(Debug)]
pub(crate) enum VapidError {
    Load(VapidLoadError),
    Lock(LockError),
    Write(AtomicWriteError),
    Sign,
}

impl std::fmt::Display for VapidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(err) => write!(f, "{err}"),
            Self::Lock(err) => write!(f, "lock error: {err}"),
            Self::Write(err) => write!(f, "write error: {err}"),
            Self::Sign => write!(f, "vapid signing error"),
        }
    }
}

impl std::error::Error for VapidError {}

impl From<VapidLoadError> for VapidError {
    fn from(err: VapidLoadError) -> Self {
        Self::Load(err)
    }
}

pub(crate) fn read_vapid_key(journal_root: &Path) -> Result<VapidKey, VapidLoadError> {
    let path = journal_root.join("config").join(VAPID_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(VapidLoadError::NotFound),
        Err(e) => {
            return Err(VapidLoadError::Unavailable {
                path,
                detail: e.to_string(),
            });
        }
    };

    let doc: VapidDocument =
        serde_json::from_str(&contents).map_err(|e| VapidLoadError::Unavailable {
            path: path.clone(),
            detail: e.to_string(),
        })?;

    let pkcs8_der =
        URL_SAFE_NO_PAD
            .decode(&doc.pkcs8)
            .map_err(|e| VapidLoadError::Unavailable {
                path: path.clone(),
                detail: format!("invalid base64url pkcs8: {e}"),
            })?;

    let key_pair = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &pkcs8_der,
        &SystemRandom::new(),
    )
    .map_err(|e| VapidLoadError::Unavailable {
        path: path.clone(),
        detail: format!("invalid ecdsa pkcs8: {e}"),
    })?;

    let pub_bytes = key_pair.public_key().as_ref();
    let public_key_uncompressed: [u8; 65] =
        pub_bytes
            .try_into()
            .map_err(|_| VapidLoadError::Unavailable {
                path: path.clone(),
                detail: "public key is not 65 bytes".to_owned(),
            })?;

    if public_key_uncompressed[0] != 0x04 {
        return Err(VapidLoadError::Unavailable {
            path,
            detail: "public key does not start with 0x04".to_owned(),
        });
    }

    Ok(VapidKey {
        key_pair,
        public_key_uncompressed,
    })
}

pub(crate) fn create_vapid_key(
    journal_root: &Path,
    created_at: String,
) -> Result<VapidKey, VapidError> {
    let path = journal_root.join("config").join(VAPID_FILE);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(OWNER_ONLY_MODE),
            ..LockOptions::default()
        },
    )
    .map_err(VapidError::Lock)?;

    // Re-check if already created under lock
    match read_vapid_key(journal_root) {
        Ok(existing) => return Ok(existing),
        Err(VapidLoadError::NotFound) => {}
        Err(e) => return Err(VapidError::Load(e)),
    }

    let rng = SystemRandom::new();
    let pkcs8_doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
        .map_err(|_| VapidError::Sign)?;

    let pkcs8_b64 = URL_SAFE_NO_PAD.encode(pkcs8_doc.as_ref());

    let doc = VapidDocument {
        pkcs8: pkcs8_b64,
        created_at,
    };

    write_json(
        &path,
        &doc,
        JsonWriteOptions {
            mode: Some(OWNER_ONLY_MODE),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(VapidError::Write)?;

    // Load back the newly written key
    read_vapid_key(journal_root).map_err(VapidError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn vapid_key_read_create_and_lock_race() {
        let root = TempDir::new_in("/var/tmp").unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();

        // 1. Basic create, read, and JWT verification
        let key1 = create_vapid_key(root.path(), "2026-09-24T00:00:00Z".to_owned()).unwrap();
        let pub1 = key1.public_key_base64url();

        let key2 = read_vapid_key(root.path()).unwrap();
        assert_eq!(key2.public_key_base64url(), pub1);

        let jwt = key1
            .sign_jwt("https://push.example:8443", 1_700_000_000)
            .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header_bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
        assert_eq!(
            std::str::from_utf8(&header_bytes).unwrap(),
            r#"{"typ":"JWT","alg":"ES256"}"#
        );

        let claims_bytes = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        assert_eq!(
            std::str::from_utf8(&claims_bytes).unwrap(),
            r#"{"aud":"https://push.example:8443","exp":1700043200}"#
        );

        let sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let peer_pub = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_FIXED,
            key1.public_key_uncompressed(),
        );
        peer_pub
            .verify(signing_input.as_bytes(), &sig_bytes)
            .expect("verify signature");

        // 2. Lock race test:
        // A fresh journal where push-vapid.json does not exist.
        // A thread holds hold_lock on config/push-vapid.json before the GET.
        // The hook runs after the handler has seen the file absent and before create_vapid_key takes the lock.
        // The hook's side writes a valid key file and releases.
        // The GET returns that public key and the file bytes stay the ones the hook wrote.
        let race_root = TempDir::new_in("/var/tmp").unwrap();
        let race_vapid_path = race_root.path().join("config").join(VAPID_FILE);
        fs::create_dir_all(race_root.path().join("config")).unwrap();
        let app = crate::api_router(race_root.path(), "https://portal.example");

        let (lock_held_tx, lock_held_rx) = std::sync::mpsc::channel();
        let (write_and_release_tx, write_and_release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (written_pubkey_tx, written_pubkey_rx) = std::sync::mpsc::channel();

        let race_root_path = race_root.path().to_path_buf();
        let race_vapid_path_clone = race_vapid_path.clone();

        let lock_thread = std::thread::spawn(move || {
            let lock = hold_lock(
                &race_vapid_path_clone,
                LockOptions {
                    mode: Some(OWNER_ONLY_MODE),
                    ..LockOptions::default()
                },
            )
            .expect("hold lock");
            lock_held_tx.send(()).unwrap();

            write_and_release_rx.recv().unwrap();

            let pkcs8_doc = EcdsaKeyPair::generate_pkcs8(
                &ECDSA_P256_SHA256_FIXED_SIGNING,
                &SystemRandom::new(),
            )
            .expect("generate pkcs8");
            let pkcs8_b64 = URL_SAFE_NO_PAD.encode(pkcs8_doc.as_ref());
            let doc = VapidDocument {
                pkcs8: pkcs8_b64,
                created_at: "2026-09-24T00:00:00Z".to_owned(),
            };
            write_json(
                &race_vapid_path_clone,
                &doc,
                JsonWriteOptions {
                    mode: Some(OWNER_ONLY_MODE),
                    ..JsonWriteOptions::default()
                },
            )
            .expect("write json on thread");

            let written_key = read_vapid_key(&race_root_path).expect("read written key");
            let pubkey = written_key.public_key_base64url();
            written_pubkey_tx.send(pubkey).unwrap();

            drop(lock);
            done_tx.send(()).unwrap();
        });

        lock_held_rx.recv().unwrap();

        let write_and_release_tx = std::sync::Mutex::new(Some(write_and_release_tx));
        let done_rx = std::sync::Mutex::new(Some(done_rx));
        set_test_absent_hook(Some(Box::new(move || {
            if let Some(tx) = write_and_release_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            if let Some(rx) = done_rx.lock().unwrap().take() {
                let _ = rx.recv();
            }
        })));

        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/api/push/vapid-key")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let get_pubkey = body_json["public_key"].as_str().unwrap();

        lock_thread.join().unwrap();
        set_test_absent_hook(None);

        let expected_pubkey = written_pubkey_rx.recv().unwrap();
        assert_eq!(get_pubkey, expected_pubkey);

        let file_key = read_vapid_key(race_root.path()).unwrap();
        assert_eq!(file_key.public_key_base64url(), expected_pubkey);
    }
}
