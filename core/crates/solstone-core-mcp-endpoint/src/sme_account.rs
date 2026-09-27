// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The solstone.me door's ACME account URL.
//!
//! The account service pins each solstone.me address's CAA record to the
//! journal's own ACME account, so every address request carries that account's
//! URL as a signed claim. The URL is learned by registering the door's account
//! key with the certificate authority's directory: registering a key the
//! directory already knows returns that account's URL, which is the same call
//! the certificate renewal makes before every order. The key is the one the
//! renewal uses (its account cache reads the same file), created here first if
//! it does not exist yet.

use std::future::Future;

use crate::McpEndpointCertificateEnvironment;
use crate::unix::{TlsStateDirectory, persist_tls_acme_account_bytes, read_tls_acme_account_bytes};

const PRODUCTION_ACCOUNT_PREFIX: &str = "https://acme-v02.api.letsencrypt.org/acme/acct/";
const STAGING_ACCOUNT_PREFIX: &str = "https://acme-staging-v02.api.letsencrypt.org/acme/acct/";

/// Payload-free failure establishing the door's account URL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SmeAccountError {
    /// The TLS state directory or key file could not be read or written.
    State,
    /// The stored key is not a usable account key, or none could be made.
    Key,
    /// The directory could not be reached or refused the registration.
    Register,
    /// The directory answered with an account URL of an unexpected shape.
    Shape,
}

pub(crate) fn is_production(environment: McpEndpointCertificateEnvironment) -> bool {
    !matches!(environment, McpEndpointCertificateEnvironment::Staging)
}

/// An account URL the account service accepts for this environment.
pub(crate) fn account_uri_has_expected_shape(uri: &str, production: bool) -> bool {
    let prefix = if production {
        PRODUCTION_ACCOUNT_PREFIX
    } else {
        STAGING_ACCOUNT_PREFIX
    };
    uri.strip_prefix(prefix).is_some_and(|digits| {
        (1..=20).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Load the door's account key for this environment, creating and storing one
/// if none exists, then learn its account URL through `register`.
pub(crate) async fn establish_account_uri<R, F>(
    directory: &TlsStateDirectory,
    production: bool,
    register: R,
) -> Result<String, SmeAccountError>
where
    R: FnOnce(Vec<u8>) -> F,
    F: Future<Output = Result<String, String>>,
{
    let key = match read_tls_acme_account_bytes(directory, production)
        .map_err(|_| SmeAccountError::State)?
    {
        Some(bytes) => {
            crate::tls::validate_acme_account_key(&bytes).map_err(|_| SmeAccountError::Key)?;
            bytes
        }
        None => {
            let keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .map_err(|_| SmeAccountError::Key)?;
            let der = keypair.serialize_der();
            persist_tls_acme_account_bytes(directory, production, &der)
                .map_err(|_| SmeAccountError::State)?;
            der
        }
    };
    let uri = register(key).await.map_err(|_| SmeAccountError::Register)?;
    if !account_uri_has_expected_shape(&uri, production) {
        return Err(SmeAccountError::Shape);
    }
    Ok(uri)
}

/// Register (or find) the account for `key` at the environment's directory.
pub(crate) async fn register_with_directory(
    key: Vec<u8>,
    production: bool,
) -> Result<String, String> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = std::sync::Arc::new(
        rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(root_store)
        .with_no_client_auth(),
    );
    let directory_url = if production {
        rustls_acme::acme::LETS_ENCRYPT_PRODUCTION_DIRECTORY
    } else {
        rustls_acme::acme::LETS_ENCRYPT_STAGING_DIRECTORY
    };
    let directory = rustls_acme::acme::Directory::discover(&client_config, directory_url)
        .await
        .map_err(|e| e.to_string())?;
    let empty_contact: [String; 0] = [];
    let account = rustls_acme::acme::Account::create_with_keypair(
        &client_config,
        directory,
        empty_contact.iter(),
        &key,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(account.kid)
}

#[cfg(all(test, not(feature = "full-tests")))]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn tls_directory() -> (tempfile::TempDir, TlsStateDirectory) {
        let temp = tempfile::TempDir::new_in(crate::test_scratch()).expect("temp");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            r#"{"mcp_endpoint":{"enabled":true}}"#,
        )
        .expect("journal config");
        let root = solstone_core_journal_io::journal_root::JournalRoot::open(temp.path())
            .expect("journal root");
        let directory = crate::unix::open_tls_state_directory(&root).expect("tls directory");
        (temp, directory)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
    }

    const PROD_URI: &str = "https://acme-v02.api.letsencrypt.org/acme/acct/123456";

    #[test]
    fn a_missing_key_is_created_where_the_renewal_reads_it_and_registered() {
        let (_temp, directory) = tls_directory();
        let seen = Arc::new(Mutex::new(None));
        let seen_in = Arc::clone(&seen);
        let uri = runtime()
            .block_on(establish_account_uri(&directory, true, |key| async move {
                *seen_in.lock().unwrap() = Some(key);
                Ok(PROD_URI.to_owned())
            }))
            .expect("established");
        assert_eq!(uri, PROD_URI);
        let stored = read_tls_acme_account_bytes(&directory, true)
            .expect("read")
            .expect("key stored for the renewal");
        assert_eq!(seen.lock().unwrap().as_deref(), Some(stored.as_slice()));
        crate::tls::validate_acme_account_key(&stored).expect("a usable account key");
        assert!(
            read_tls_acme_account_bytes(&directory, false)
                .expect("read")
                .is_none(),
            "the other environment's key is untouched"
        );
    }

    #[test]
    fn an_existing_key_is_registered_as_is_and_never_replaced() {
        let (_temp, directory) = tls_directory();
        let existing = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .expect("key")
            .serialize_der();
        persist_tls_acme_account_bytes(&directory, true, &existing).expect("seed");
        let seen = Arc::new(Mutex::new(None));
        let seen_in = Arc::clone(&seen);
        runtime()
            .block_on(establish_account_uri(&directory, true, |key| async move {
                *seen_in.lock().unwrap() = Some(key);
                Ok(PROD_URI.to_owned())
            }))
            .expect("established");
        assert_eq!(seen.lock().unwrap().as_deref(), Some(existing.as_slice()));
        assert_eq!(
            read_tls_acme_account_bytes(&directory, true)
                .expect("read")
                .as_deref(),
            Some(existing.as_slice())
        );
    }

    #[test]
    fn a_failed_registration_or_an_unexpected_url_is_an_error_and_keeps_the_key() {
        let (_temp, directory) = tls_directory();
        let failed = runtime().block_on(establish_account_uri(&directory, true, |_| async {
            Err::<String, String>("unreachable".to_owned())
        }));
        assert_eq!(failed, Err(SmeAccountError::Register));
        let kept = read_tls_acme_account_bytes(&directory, true)
            .expect("read")
            .expect("key kept");
        for bad in [
            "https://acme-staging-v02.api.letsencrypt.org/acme/acct/1",
            "https://acme-v02.api.letsencrypt.org/acme/acct/",
            "https://acme-v02.api.letsencrypt.org/acme/acct/12a",
            "http://acme-v02.api.letsencrypt.org/acme/acct/1",
            "https://acme-v02.api.letsencrypt.org/acme/acct/1/",
        ] {
            let result =
                runtime().block_on(establish_account_uri(&directory, true, |_| async move {
                    Ok(bad.to_owned())
                }));
            assert_eq!(result, Err(SmeAccountError::Shape), "{bad}");
        }
        assert_eq!(
            read_tls_acme_account_bytes(&directory, true).expect("read"),
            Some(kept),
            "a failure never replaces the key"
        );
    }

    #[test]
    fn an_unusable_stored_key_is_refused_not_replaced() {
        let (_temp, directory) = tls_directory();
        persist_tls_acme_account_bytes(&directory, true, b"not a key").expect("seed");
        let result = runtime().block_on(establish_account_uri(&directory, true, |_| async {
            Ok(PROD_URI.to_owned())
        }));
        assert_eq!(result, Err(SmeAccountError::Key));
        assert_eq!(
            read_tls_acme_account_bytes(&directory, true)
                .expect("read")
                .as_deref(),
            Some(&b"not a key"[..])
        );
    }

    #[test]
    fn account_url_shapes_match_the_environment() {
        assert!(account_uri_has_expected_shape(PROD_URI, true));
        assert!(!account_uri_has_expected_shape(PROD_URI, false));
        assert!(account_uri_has_expected_shape(
            "https://acme-staging-v02.api.letsencrypt.org/acme/acct/344189013",
            false
        ));
        assert!(!account_uri_has_expected_shape(
            "https://acme-v02.api.letsencrypt.org/acme/acct/123456789012345678901",
            true
        ));
    }
}
