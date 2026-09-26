// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-held, hot-swappable TLS state for the MCP listener.
//!
//! Lane B owns the listener. This module owns the exact TLS 1.3 resolver it
//! receives, the ordinary certificate state below the journal root, and the
//! short-lived in-memory TLS-ALPN challenge certificate. No caller can supply
//! a certificate path or recover a private key from the opaque service.

use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration as StdDuration, Instant};

use arc_swap::ArcSwapOption;
use base64::Engine as _;
use futures::StreamExt as _;
use rcgen::{
    CertificateParams, CustomExtension, DistinguishedName, KeyPair, KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256,
};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls_acme::{
    AccountCache, AcmeConfig, CertCache, EventError, OrderError, ResolvesServerCertAcme,
};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use solstone_core_journal_config::McpEndpointCertificateEnvironment;
use solstone_core_journal_io::journal_root::JournalRoot;
use time::{Duration, OffsetDateTime};
use x509_parser::prelude::{GeneralName, parse_x509_certificate};

use crate::unix::{self, TlsStateDirectory};

const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";
const HTTP11_ALPN: &[u8] = b"http/1.1";
const ACME_IDENTIFIER_OID: &[u64] = &[1, 3, 6, 1, 5, 5, 7, 1, 31];

/// Opaque, owner-held certificate state for one account-authorized MCP name.
///
/// The construction, persistent store, certificate installation, and challenge
/// routes are crate-private. Consumers can only hand this handle to
/// [`mcp_endpoint_server_config`].
pub struct McpEndpointTlsService {
    resolver: Arc<McpEndpointCertificateResolver>,
    #[allow(dead_code)] // Reserved for the same-crate ACME lifecycle owner.
    store: Option<Arc<CertificateStateStore>>,
    force_staging_renewal: bool,
}

struct CertificateStateStore {
    directory: TlsStateDirectory,
    #[allow(dead_code)] // Read by the same-crate ordinary-certificate installer.
    environment: McpEndpointCertificateEnvironment,
}

struct McpEndpointCertificateResolver {
    hostname: String,
    ordinary: ArcSwapOption<ActiveOrdinaryCertificate>,
    challenge: ArcSwapOption<ActiveChallengeCertificate>,
    acme: ArcSwapOption<ResolvesServerCertAcme>,
    #[allow(dead_code)] // Consumed by the same-crate challenge lifecycle owner.
    next_challenge_generation: AtomicU64,
}

struct ActiveOrdinaryCertificate {
    key: Arc<CertifiedKey>,
    expires_at: Instant,
    not_after: i64,
}

#[cfg(test)]
pub static TEST_NOW_OVERRIDE: std::sync::RwLock<Option<time::OffsetDateTime>> =
    std::sync::RwLock::new(None);

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn set_test_now_override(timestamp: Option<i64>) {
    if let Ok(mut guard) = TEST_NOW_OVERRIDE.write() {
        *guard = timestamp.and_then(|ts| time::OffsetDateTime::from_unix_timestamp(ts).ok());
    }
}

pub(crate) fn current_epoch_seconds() -> i64 {
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_NOW_OVERRIDE.read()
            && let Some(now) = *guard
        {
            return now.unix_timestamp();
        }
    }
    time::OffsetDateTime::now_utc().unix_timestamp()
}

struct ActiveChallengeCertificate {
    key: Arc<CertifiedKey>,
    generation: u64,
}

/// A scoped handle for an active TLS-ALPN-01 challenge certificate.
///
/// It clears only its own generation. Dropping an old guard therefore cannot
/// withdraw a challenge installed by a later issuance attempt.
#[allow(dead_code)] // Constructed by the ACME lifecycle owner in this crate.
pub(crate) struct McpEndpointChallengeGuard {
    resolver: Arc<McpEndpointCertificateResolver>,
    generation: u64,
}

/// Payload-free result from the unattended ACME certificate lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpEndpointCertificateLifecycleError {
    /// Owner-held TLS state could not be opened or validated.
    State,
}

impl fmt::Display for McpEndpointCertificateLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MCP endpoint certificate lifecycle could not start")
    }
}

impl std::error::Error for McpEndpointCertificateLifecycleError {}

struct McpEndpointAcmeCache {
    service: McpEndpointTlsService,
    production: bool,
    force_staging_renewal: bool,
}

struct McpEndpointAcmeResolverGuard {
    resolver: Arc<McpEndpointCertificateResolver>,
    installed: Arc<ResolvesServerCertAcme>,
}

/// Payload-free failure from local TLS state validation or activation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum McpEndpointTlsError {
    State,
}

impl fmt::Debug for McpEndpointCertificateResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpEndpointCertificateResolver")
            .finish_non_exhaustive()
    }
}

impl McpEndpointTlsService {
    pub(crate) fn ordinary_certificate_is_active(&self) -> bool {
        let now_ts = current_epoch_seconds();
        self.resolver
            .ordinary
            .load_full()
            .as_deref()
            .is_some_and(|ordinary| {
                now_ts < ordinary.not_after && Instant::now() < ordinary.expires_at
            })
    }
    pub(crate) fn authorized_hostname(&self) -> &str {
        &self.resolver.hostname
    }

    /// Construct TLS state for an account-authorized hostname and activate a
    /// valid persisted ordinary certificate, if one exists. Invalid state is
    /// never repaired or replaced during load.
    pub(crate) fn for_authorized_hostname(
        journal_root: Arc<JournalRoot>,
        hostname: String,
        environment: McpEndpointCertificateEnvironment,
        force_staging_renewal: bool,
    ) -> Result<Self, McpEndpointTlsError> {
        if !is_exact_authorized_hostname(&hostname, &hostname)
            || (force_staging_renewal
                && matches!(environment, McpEndpointCertificateEnvironment::Production))
        {
            return Err(McpEndpointTlsError::State);
        }
        let store = Arc::new(CertificateStateStore {
            directory: unix::open_tls_state_directory(&journal_root)
                .map_err(|_| McpEndpointTlsError::State)?,
            environment,
        });
        let service = Self::empty(hostname, Some(Arc::clone(&store)), force_staging_renewal);
        if let Some(bytes) =
            unix::read_tls_state_bytes(&store.directory).map_err(|_| McpEndpointTlsError::State)?
        {
            match classify_stored_state(&bytes, &service.resolver.hostname, environment)? {
                StoredCertificateClassification::Same(decoded) => {
                    if certificate_is_current(&decoded)? {
                        let active = activate_decoded_stored_state(decoded)?;
                        service.resolver.ordinary.store(Some(Arc::new(active)));
                    }
                }
                StoredCertificateClassification::Foreign => {}
            }
        }
        Ok(service)
    }

    pub(crate) fn for_byo_cert_directory(
        cert_directory: unix::TlsStateDirectory,
        hostname: String,
    ) -> Result<Self, McpEndpointTlsError> {
        if !is_exact_authorized_hostname(&hostname, &hostname) {
            return Err(McpEndpointTlsError::State);
        }
        let store = Arc::new(CertificateStateStore {
            directory: cert_directory,
            environment: McpEndpointCertificateEnvironment::Production,
        });
        let service = Self::empty(hostname, Some(Arc::clone(&store)), false);
        if let Some(bytes) =
            unix::read_tls_state_bytes(&store.directory).map_err(|_| McpEndpointTlsError::State)?
        {
            match classify_stored_state(
                &bytes,
                &service.resolver.hostname,
                McpEndpointCertificateEnvironment::Production,
            )? {
                StoredCertificateClassification::Same(decoded) => {
                    if certificate_is_current(&decoded)? {
                        let active = activate_decoded_stored_state(decoded)?;
                        service.resolver.ordinary.store(Some(Arc::new(active)));
                    }
                }
                StoredCertificateClassification::Foreign => {}
            }
        }
        Ok(service)
    }

    /// Whether a newly authenticated bridge authority names this service's
    /// exact certificate hostname. This stays crate-private so callers cannot
    /// inspect or select the hostname themselves.
    pub(crate) fn matches_authorized_hostname(&self, hostname: &str) -> bool {
        is_exact_authorized_hostname(hostname, &self.resolver.hostname)
    }

    fn empty(
        hostname: String,
        store: Option<Arc<CertificateStateStore>>,
        force_staging_renewal: bool,
    ) -> Self {
        Self {
            resolver: Arc::new(McpEndpointCertificateResolver {
                hostname,
                ordinary: ArcSwapOption::empty(),
                challenge: ArcSwapOption::empty(),
                acme: ArcSwapOption::empty(),
                next_challenge_generation: AtomicU64::new(0),
            }),
            store,
            force_staging_renewal,
        }
    }

    /// Validate, persist, reopen, revalidate, then activate a newly issued
    /// ordinary certificate. A persistence or reopen failure leaves the prior
    /// still-valid key active.
    #[allow(dead_code)] // The lifecycle owner is added separately from TLS state.
    pub(crate) fn install_ordinary_certificate(
        &self,
        certificate_chain: Vec<Vec<u8>>,
        private_key: Vec<u8>,
        not_before: i64,
        not_after: i64,
    ) -> Result<(), McpEndpointTlsError> {
        let store = self.store.as_ref().ok_or(McpEndpointTlsError::State)?;
        let state = StoredCertificateState {
            environment: environment_name(store.environment).to_owned(),
            hostname: self.resolver.hostname.clone(),
            certificate_chain: certificate_chain
                .into_iter()
                .map(|value| base64::engine::general_purpose::STANDARD.encode(value))
                .collect(),
            private_key: base64::engine::general_purpose::STANDARD.encode(private_key),
            not_before,
            not_after,
        };
        let serialized = serde_json::to_vec(&state).map_err(|_| McpEndpointTlsError::State)?;
        let _ = validate_stored_state(&serialized, &self.resolver.hostname, store.environment)?;
        unix::persist_tls_state_bytes(&store.directory, &serialized)
            .map_err(|_| McpEndpointTlsError::State)?;
        let reopened = unix::read_tls_state_bytes(&store.directory)
            .map_err(|_| McpEndpointTlsError::State)?
            .ok_or(McpEndpointTlsError::State)?;
        let active = validate_stored_state(&reopened, &self.resolver.hostname, store.environment)?;
        self.resolver.ordinary.store(Some(Arc::new(active)));
        Ok(())
    }

    /// Install a short-lived RFC 8737 certificate for one ACME key-authorization
    /// digest. The certificate and private key exist only in this process.
    #[allow(dead_code)] // The lifecycle owner is added separately from TLS state.
    pub(crate) fn install_acme_challenge(
        &self,
        key_authorization_digest: [u8; 32],
    ) -> Result<McpEndpointChallengeGuard, McpEndpointTlsError> {
        let key = build_acme_challenge_key(&self.resolver.hostname, &key_authorization_digest)?;
        let generation = self
            .resolver
            .next_challenge_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let generation = if generation == 0 { 1 } else { generation };
        self.resolver
            .challenge
            .store(Some(Arc::new(ActiveChallengeCertificate {
                key,
                generation,
            })));
        Ok(McpEndpointChallengeGuard {
            resolver: Arc::clone(&self.resolver),
            generation,
        })
    }

    /// Keep the journal-owned ACME account and certificate current until the
    /// supplied listener shutdown. The caller must already be serving this
    /// service's [`mcp_endpoint_server_config`]: TLS-ALPN-01 validation is
    /// deliberately answered by that same listener, not by a second port.
    ///
    /// A cached but expired certificate is structurally validated and passed
    /// to the ACME scheduler without being activated for ordinary handshakes.
    /// Consequently a journal returning after an offline renewal window
    /// immediately retries issuance while never serving an expired key.
    pub async fn run_acme_renewal(
        &self,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), McpEndpointCertificateLifecycleError> {
        self.run_acme_renewal_inner(None, shutdown).await
    }

    pub(crate) async fn run_acme_renewal_with_owner_state(
        &self,
        journal_root: &Path,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), McpEndpointCertificateLifecycleError> {
        self.run_acme_renewal_inner(Some(journal_root), shutdown)
            .await
    }

    async fn run_acme_renewal_inner(
        &self,
        journal_root: Option<&Path>,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), McpEndpointCertificateLifecycleError> {
        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            return Ok(());
        }
        let store = self
            .store
            .as_ref()
            .ok_or(McpEndpointCertificateLifecycleError::State)?;
        let production = matches!(
            store.environment,
            McpEndpointCertificateEnvironment::Production
        );
        let cache = McpEndpointAcmeCache {
            service: self.lifecycle_copy(),
            production,
            force_staging_renewal: self.force_staging_renewal,
        };
        let mut state = AcmeConfig::new([self.resolver.hostname.as_str()])
            .cache(cache)
            .directory_lets_encrypt(production)
            .state();
        let _resolver_guard = McpEndpointAcmeResolverGuard {
            resolver: Arc::clone(&self.resolver),
            installed: state.resolver(),
        };
        self.resolver
            .acme
            .store(Some(Arc::clone(&_resolver_guard.installed)));

        let mut retry_count = 0_u32;
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow_and_update() {
                        return Ok(());
                    }
                }
                event = state.next() => match event {
                    Some(Ok(_)) => { retry_count = 0; },
                    Some(Err(error)) if persistent_acme_state_error(&error) => {
                        return Err(McpEndpointCertificateLifecycleError::State);
                    }
                    Some(Err(error)) => {
                        if acme_rate_limited(&error) {
                            let delay = 1_i64 << retry_count.min(16);
                            if let Some(root) = journal_root {
                                crate::owner_state::write_mcp_rate_limit_state(
                                    root,
                                    &self.resolver.hostname,
                                    chrono::Utc::now() + chrono::Duration::seconds(delay),
                                );
                            }
                        }
                        retry_count = retry_count.saturating_add(1);
                    },
                    None => return Err(McpEndpointCertificateLifecycleError::State),
                }
            }
        }
    }

    pub(crate) async fn run_byo_acme_renewal(
        &self,
        account_dir: unix::TlsStateDirectory,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), McpEndpointCertificateLifecycleError> {
        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            return Ok(());
        }
        let _store = self
            .store
            .as_ref()
            .ok_or(McpEndpointCertificateLifecycleError::State)?;
        let cache = ByoAccountCache {
            account_dir,
            service: self.lifecycle_copy(),
        };
        let mut state = AcmeConfig::new([self.resolver.hostname.as_str()])
            .cache(cache)
            .directory_lets_encrypt(true)
            .state();
        let _resolver_guard = McpEndpointAcmeResolverGuard {
            resolver: Arc::clone(&self.resolver),
            installed: state.resolver(),
        };
        self.resolver
            .acme
            .store(Some(Arc::clone(&_resolver_guard.installed)));

        let mut retry_count = 0_u32;
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow_and_update() {
                        return Ok(());
                    }
                }
                event = state.next() => match event {
                    Some(Ok(_)) => { retry_count = 0; },
                    Some(Err(error)) if persistent_acme_state_error(&error) => {
                        return Err(McpEndpointCertificateLifecycleError::State);
                    }
                    Some(Err(_)) => {
                        retry_count = retry_count.saturating_add(1);
                    },
                    None => return Err(McpEndpointCertificateLifecycleError::State),
                }
            }
        }
    }

    fn lifecycle_copy(&self) -> Self {
        Self {
            resolver: Arc::clone(&self.resolver),
            store: self.store.as_ref().map(Arc::clone),
            force_staging_renewal: self.force_staging_renewal,
        }
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    pub(crate) fn empty_for_test(hostname: String) -> Self {
        Self::empty(hostname, None, false)
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    fn install_ordinary_for_test(&self, key: Arc<CertifiedKey>) {
        self.resolver
            .ordinary
            .store(Some(Arc::new(ActiveOrdinaryCertificate {
                key,
                expires_at: Instant::now() + StdDuration::from_secs(3600),
                not_after: time::OffsetDateTime::now_utc().unix_timestamp() + 3600,
            })));
    }
}

fn acme_rate_limited<EC: std::fmt::Debug, EA: std::fmt::Debug>(error: &EventError<EC, EA>) -> bool {
    match error {
        EventError::Order(OrderError::BadOrder(order)) => order
            .error
            .as_ref()
            .and_then(|problem| problem.typ.as_deref())
            .is_some_and(|kind| kind.ends_with(":rateLimited")),
        _ => false,
    }
}

fn persistent_acme_state_error(error: &EventError<io::Error, io::Error>) -> bool {
    matches!(
        error,
        EventError::CertCacheLoad(_)
            | EventError::AccountCacheLoad(_)
            | EventError::CertCacheStore(_)
            | EventError::AccountCacheStore(_)
            | EventError::CachedCertParse(_)
    )
}

impl Drop for McpEndpointAcmeResolverGuard {
    fn drop(&mut self) {
        let current = self.resolver.acme.load_full();
        if current
            .as_ref()
            .is_some_and(|resolver| Arc::ptr_eq(resolver, &self.installed))
        {
            let _ = self.resolver.acme.compare_and_swap(&current, None);
        }
    }
}

#[async_trait::async_trait]
impl CertCache for McpEndpointAcmeCache {
    type EC = io::Error;

    async fn load_cert(
        &self,
        _domains: &[String],
        _directory_url: &str,
    ) -> Result<Option<Vec<u8>>, Self::EC> {
        if self.force_staging_renewal {
            return Ok(None);
        }
        let store = self
            .service
            .store
            .as_ref()
            .ok_or_else(|| io::Error::other("MCP endpoint TLS state is unavailable"))?;
        let Some(bytes) = unix::read_tls_state_bytes(&store.directory)? else {
            return Ok(None);
        };
        match classify_stored_state(&bytes, &self.service.resolver.hostname, store.environment)
            .map_err(|_| io::Error::other("MCP endpoint certificate state is invalid"))?
        {
            StoredCertificateClassification::Same(decoded) => {
                Ok(Some(stored_state_to_pem(decoded)))
            }
            StoredCertificateClassification::Foreign => Ok(None),
        }
    }

    async fn store_cert(
        &self,
        _domains: &[String],
        _directory_url: &str,
        certificate: &[u8],
    ) -> Result<(), Self::EC> {
        let issued = issued_pem_parts(certificate)
            .map_err(|_| io::Error::other("ACME returned an invalid certificate bundle"))?;
        self.service
            .install_ordinary_certificate(
                issued.certificate_chain,
                issued.private_key,
                issued.not_before,
                issued.not_after,
            )
            .map_err(|_| io::Error::other("ACME certificate state could not be activated"))
    }
}

#[async_trait::async_trait]
impl AccountCache for McpEndpointAcmeCache {
    type EA = io::Error;

    async fn load_account(
        &self,
        _contact: &[String],
        _directory_url: &str,
    ) -> Result<Option<Vec<u8>>, Self::EA> {
        let store = self
            .service
            .store
            .as_ref()
            .ok_or_else(|| io::Error::other("MCP endpoint TLS state is unavailable"))?;
        let account = unix::read_tls_acme_account_bytes(&store.directory, self.production)?;
        if let Some(account) = account.as_deref() {
            validate_acme_account_key(account)
                .map_err(|_| io::Error::other("MCP endpoint ACME account state is invalid"))?;
        }
        Ok(account)
    }

    async fn store_account(
        &self,
        _contact: &[String],
        _directory_url: &str,
        account: &[u8],
    ) -> Result<(), Self::EA> {
        validate_acme_account_key(account)
            .map_err(|_| io::Error::other("ACME returned an invalid account key"))?;
        let store = self
            .service
            .store
            .as_ref()
            .ok_or_else(|| io::Error::other("MCP endpoint TLS state is unavailable"))?;
        unix::persist_tls_acme_account_bytes(&store.directory, self.production, account)
    }
}

pub(crate) struct ByoAccountCache {
    pub(crate) account_dir: unix::TlsStateDirectory,
    pub(crate) service: McpEndpointTlsService,
}

#[async_trait::async_trait]
impl CertCache for ByoAccountCache {
    type EC = io::Error;

    async fn load_cert(
        &self,
        _domains: &[String],
        _directory_url: &str,
    ) -> Result<Option<Vec<u8>>, Self::EC> {
        let store = self
            .service
            .store
            .as_ref()
            .ok_or_else(|| io::Error::other("MCP endpoint TLS state is unavailable"))?;
        let Some(bytes) = unix::read_tls_state_bytes(&store.directory)? else {
            return Ok(None);
        };
        match classify_stored_state(
            &bytes,
            &self.service.resolver.hostname,
            McpEndpointCertificateEnvironment::Production,
        )
        .map_err(|_| io::Error::other("MCP endpoint certificate state is invalid"))?
        {
            StoredCertificateClassification::Same(decoded) => {
                Ok(Some(stored_state_to_pem(decoded)))
            }
            StoredCertificateClassification::Foreign => Ok(None),
        }
    }

    async fn store_cert(
        &self,
        _domains: &[String],
        _directory_url: &str,
        certificate: &[u8],
    ) -> Result<(), Self::EC> {
        let issued = issued_pem_parts(certificate)
            .map_err(|_| io::Error::other("ACME returned an invalid certificate bundle"))?;
        self.service
            .install_ordinary_certificate(
                issued.certificate_chain,
                issued.private_key,
                issued.not_before,
                issued.not_after,
            )
            .map_err(|_| io::Error::other("ACME certificate state could not be activated"))
    }
}

#[async_trait::async_trait]
impl AccountCache for ByoAccountCache {
    type EA = io::Error;

    async fn load_account(
        &self,
        _contact: &[String],
        _directory_url: &str,
    ) -> Result<Option<Vec<u8>>, Self::EA> {
        let uri = unix::read_byo_account_uri(&self.account_dir)?;
        if uri.is_none() {
            return Err(io::Error::other("account not registered"));
        }
        let account = unix::read_byo_account_key(&self.account_dir)?;
        let Some(account_bytes) = account else {
            return Err(io::Error::other("account key missing"));
        };
        validate_acme_account_key(&account_bytes)
            .map_err(|_| io::Error::other("MCP endpoint ACME account key is invalid"))?;
        Ok(Some(account_bytes))
    }

    async fn store_account(
        &self,
        _contact: &[String],
        _directory_url: &str,
        account: &[u8],
    ) -> Result<(), Self::EA> {
        validate_acme_account_key(account)
            .map_err(|_| io::Error::other("ACME returned an invalid account key"))?;
        unix::persist_byo_account_key(&self.account_dir, account)
    }
}

impl Drop for McpEndpointChallengeGuard {
    fn drop(&mut self) {
        let current = self.resolver.challenge.load_full();
        if current
            .as_ref()
            .is_some_and(|challenge| challenge.generation == self.generation)
        {
            let _ = self.resolver.challenge.compare_and_swap(&current, None);
        }
    }
}

/// Build the one TLS configuration that Lane B's dedicated listener consumes.
///
/// The resolver stays shared with the service: a persisted ordinary-certificate
/// install or temporary ACME challenge affects only new handshakes, never an
/// established TLS session or the listener configuration identity.
pub fn mcp_endpoint_server_config(service: &McpEndpointTlsService) -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let resolver: Arc<dyn ResolvesServerCert> = service.resolver.clone();
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring provider supports TLS 1.3")
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    config.alpn_protocols = vec![ACME_TLS_ALPN.to_vec(), HTTP11_ALPN.to_vec()];
    Arc::new(config)
}

impl ResolvesServerCert for McpEndpointCertificateResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let server_name = hello.server_name()?;
        if !is_exact_authorized_hostname(server_name, &self.hostname) {
            return None;
        }
        let alpn = hello
            .alpn()
            .map(|protocols| protocols.collect::<Vec<_>>())
            .unwrap_or_default();
        if is_acme_attempt(&alpn) {
            if !is_only_acme_alpn(&alpn) {
                return None;
            }
            if let Some(resolver) = self.acme.load_full() {
                return resolver.resolve(hello);
            }
            return self
                .challenge
                .load_full()
                .as_deref()
                .filter(|challenge| challenge.generation != 0)
                .map(|challenge| Arc::clone(&challenge.key));
        }
        let now_ts = current_epoch_seconds();
        self.ordinary
            .load_full()
            .as_deref()
            .filter(|ordinary| now_ts < ordinary.not_after && Instant::now() < ordinary.expires_at)
            .map(|ordinary| Arc::clone(&ordinary.key))
    }
}

#[derive(Serialize)]
struct StoredCertificateState {
    environment: String,
    hostname: String,
    certificate_chain: Vec<String>,
    private_key: String,
    not_before: i64,
    not_after: i64,
}

impl<'de> Deserialize<'de> for StoredCertificateState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StateVisitor;

        impl<'de> Visitor<'de> for StateVisitor {
            type Value = StoredCertificateState;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a strict MCP endpoint certificate state object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut environment = None;
                let mut hostname = None;
                let mut certificate_chain = None;
                let mut private_key = None;
                let mut not_before = None;
                let mut not_after = None;
                while let Some(field) = map.next_key::<String>()? {
                    match field.as_str() {
                        "environment" => assign_field(&mut environment, &field, &mut map)?,
                        "hostname" => assign_field(&mut hostname, &field, &mut map)?,
                        "certificate_chain" => {
                            assign_field(&mut certificate_chain, &field, &mut map)?
                        }
                        "private_key" => assign_field(&mut private_key, &field, &mut map)?,
                        "not_before" => assign_field(&mut not_before, &field, &mut map)?,
                        "not_after" => assign_field(&mut not_after, &field, &mut map)?,
                        _ => return Err(de::Error::unknown_field(&field, STATE_FIELDS)),
                    }
                }
                Ok(StoredCertificateState {
                    environment: environment
                        .ok_or_else(|| de::Error::missing_field("environment"))?,
                    hostname: hostname.ok_or_else(|| de::Error::missing_field("hostname"))?,
                    certificate_chain: certificate_chain
                        .ok_or_else(|| de::Error::missing_field("certificate_chain"))?,
                    private_key: private_key
                        .ok_or_else(|| de::Error::missing_field("private_key"))?,
                    not_before: not_before.ok_or_else(|| de::Error::missing_field("not_before"))?,
                    not_after: not_after.ok_or_else(|| de::Error::missing_field("not_after"))?,
                })
            }
        }

        deserializer.deserialize_map(StateVisitor)
    }
}

const STATE_FIELDS: &[&str] = &[
    "environment",
    "hostname",
    "certificate_chain",
    "private_key",
    "not_before",
    "not_after",
];

fn assign_field<'de, A, T>(slot: &mut Option<T>, _field: &str, map: &mut A) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::custom("duplicate certificate state field"));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

struct DecodedStoredCertificateState {
    certificate_chain: Vec<Vec<u8>>,
    private_key: Vec<u8>,
    not_before: i64,
    not_after: i64,
}

struct IssuedCertificate {
    certificate_chain: Vec<Vec<u8>>,
    private_key: Vec<u8>,
    not_before: i64,
    not_after: i64,
}

fn validate_stored_state(
    bytes: &[u8],
    expected_hostname: &str,
    expected_environment: McpEndpointCertificateEnvironment,
) -> Result<ActiveOrdinaryCertificate, McpEndpointTlsError> {
    let decoded = decode_stored_state(bytes, expected_hostname, expected_environment)?;
    if !certificate_is_current(&decoded)? {
        return Err(McpEndpointTlsError::State);
    }
    activate_decoded_stored_state(decoded)
}

enum StoredCertificateClassification {
    Same(DecodedStoredCertificateState),
    Foreign,
}

fn classify_stored_state(
    bytes: &[u8],
    expected_hostname: &str,
    expected_environment: McpEndpointCertificateEnvironment,
) -> Result<StoredCertificateClassification, McpEndpointTlsError> {
    if bytes.len() > unix::MAX_TLS_STATE_BYTES {
        return Err(McpEndpointTlsError::State);
    }
    let state = serde_json::from_slice::<StoredCertificateState>(bytes)
        .map_err(|_| McpEndpointTlsError::State)?;
    let stored_environment = match state.environment.as_str() {
        "staging" => McpEndpointCertificateEnvironment::Staging,
        "production" => McpEndpointCertificateEnvironment::Production,
        _ => return Err(McpEndpointTlsError::State),
    };
    if state.hostname != expected_hostname
        || !is_exact_authorized_hostname(&state.hostname, expected_hostname)
        || state.not_before >= state.not_after
    {
        return Err(McpEndpointTlsError::State);
    }
    let certificate_chain = state
        .certificate_chain
        .iter()
        .map(|value| decode_base64_exact(value))
        .collect::<Result<Vec<_>, _>>()?;
    if certificate_chain.is_empty() {
        return Err(McpEndpointTlsError::State);
    }
    let private_key = decode_base64_exact(&state.private_key)?;
    let parsed = certificate_chain
        .iter()
        .map(|certificate| {
            let (remainder, parsed) =
                parse_x509_certificate(certificate).map_err(|_| McpEndpointTlsError::State)?;
            if !remainder.is_empty() {
                return Err(McpEndpointTlsError::State);
            }
            Ok(parsed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let leaf = parsed.first().ok_or(McpEndpointTlsError::State)?;
    let names = leaf
        .subject_alternative_name()
        .map_err(|_| McpEndpointTlsError::State)?
        .ok_or(McpEndpointTlsError::State)?;
    if names.value.general_names.len() != 1
        || !matches!(
            names.value.general_names.first(),
            Some(GeneralName::DNSName(name)) if *name == expected_hostname
        )
        || leaf.validity().not_before.timestamp() != state.not_before
        || leaf.validity().not_after.timestamp() != state.not_after
    {
        return Err(McpEndpointTlsError::State);
    }
    for pair in parsed.windows(2) {
        if pair[0].issuer() != pair[1].subject()
            || pair[0]
                .verify_signature(Some(pair[1].public_key()))
                .is_err()
        {
            return Err(McpEndpointTlsError::State);
        }
    }
    let ecdsa_key = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &private_key,
        &SystemRandom::new(),
    )
    .or_else(|_| {
        EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &private_key,
            &SystemRandom::new(),
        )
    })
    .map_err(|_| McpEndpointTlsError::State)?;
    if leaf.public_key().subject_public_key.data != ecdsa_key.public_key().as_ref() {
        return Err(McpEndpointTlsError::State);
    }
    if stored_environment == expected_environment {
        Ok(StoredCertificateClassification::Same(
            DecodedStoredCertificateState {
                certificate_chain,
                private_key,
                not_before: state.not_before,
                not_after: state.not_after,
            },
        ))
    } else {
        Ok(StoredCertificateClassification::Foreign)
    }
}

fn decode_stored_state(
    bytes: &[u8],
    expected_hostname: &str,
    expected_environment: McpEndpointCertificateEnvironment,
) -> Result<DecodedStoredCertificateState, McpEndpointTlsError> {
    match classify_stored_state(bytes, expected_hostname, expected_environment)? {
        StoredCertificateClassification::Same(decoded) => Ok(decoded),
        StoredCertificateClassification::Foreign => Err(McpEndpointTlsError::State),
    }
}

fn certificate_is_current(
    state: &DecodedStoredCertificateState,
) -> Result<bool, McpEndpointTlsError> {
    let now = now_unix_seconds()?;
    Ok(now >= state.not_before && now < state.not_after)
}

fn activate_decoded_stored_state(
    state: DecodedStoredCertificateState,
) -> Result<ActiveOrdinaryCertificate, McpEndpointTlsError> {
    let DecodedStoredCertificateState {
        certificate_chain,
        private_key,
        not_before: _,
        not_after,
    } = state;
    let now = now_unix_seconds()?;
    let private_key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key_der)
        .map_err(|_| McpEndpointTlsError::State)?;
    let certificate_chain = certificate_chain
        .into_iter()
        .map(CertificateDer::from)
        .collect();
    let seconds_until_expiry = not_after
        .checked_sub(now)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .filter(|seconds| *seconds > 0)
        .ok_or(McpEndpointTlsError::State)?;
    let expires_at = Instant::now()
        .checked_add(StdDuration::from_secs(seconds_until_expiry))
        .ok_or(McpEndpointTlsError::State)?;
    Ok(ActiveOrdinaryCertificate {
        key: Arc::new(CertifiedKey::new(certificate_chain, signing_key)),
        expires_at,
        not_after,
    })
}

fn stored_state_to_pem(state: DecodedStoredCertificateState) -> Vec<u8> {
    let mut blocks = Vec::with_capacity(state.certificate_chain.len() + 1);
    blocks.push(pem::Pem::new("PRIVATE KEY", state.private_key));
    blocks.extend(
        state
            .certificate_chain
            .into_iter()
            .map(|certificate| pem::Pem::new("CERTIFICATE", certificate)),
    );
    pem::encode_many(&blocks).into_bytes()
}

fn issued_pem_parts(certificate: &[u8]) -> Result<IssuedCertificate, McpEndpointTlsError> {
    let blocks = pem::parse_many(certificate).map_err(|_| McpEndpointTlsError::State)?;
    let (private_key, certificates) = blocks.split_first().ok_or(McpEndpointTlsError::State)?;
    if private_key.tag() != "PRIVATE KEY" || certificates.is_empty() {
        return Err(McpEndpointTlsError::State);
    }
    if certificates
        .iter()
        .any(|block| block.tag() != "CERTIFICATE")
    {
        return Err(McpEndpointTlsError::State);
    }
    let chain = certificates
        .iter()
        .map(|certificate| certificate.contents().to_vec())
        .collect::<Vec<_>>();
    let (remainder, leaf) =
        parse_x509_certificate(&chain[0]).map_err(|_| McpEndpointTlsError::State)?;
    if !remainder.is_empty() {
        return Err(McpEndpointTlsError::State);
    }
    let not_before = leaf.validity().not_before.timestamp();
    let not_after = leaf.validity().not_after.timestamp();
    Ok(IssuedCertificate {
        certificate_chain: chain,
        private_key: private_key.contents().to_vec(),
        not_before,
        not_after,
    })
}

pub(crate) fn validate_acme_account_key(bytes: &[u8]) -> Result<(), McpEndpointTlsError> {
    if bytes.is_empty() || bytes.len() > unix::MAX_TLS_ACME_ACCOUNT_BYTES {
        return Err(McpEndpointTlsError::State);
    }
    EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        bytes,
        &SystemRandom::new(),
    )
    .or_else(|_| {
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, bytes, &SystemRandom::new())
    })
    .map(|_| ())
    .map_err(|_| McpEndpointTlsError::State)
}

#[allow(dead_code)] // Called by the same-crate ACME lifecycle owner.
fn build_acme_challenge_key(
    hostname: &str,
    key_authorization_digest: &[u8; 32],
) -> Result<Arc<CertifiedKey>, McpEndpointTlsError> {
    let now = OffsetDateTime::now_utc();
    let key_pair =
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|_| McpEndpointTlsError::State)?;
    let mut params = CertificateParams::new(vec![hostname.to_owned()])
        .map_err(|_| McpEndpointTlsError::State)?;
    params.distinguished_name = DistinguishedName::new();
    params.not_before = now - Duration::minutes(1);
    params.not_after = now + Duration::minutes(10);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let mut extension = CustomExtension::from_oid_content(
        ACME_IDENTIFIER_OID,
        der_octet_string(key_authorization_digest),
    );
    extension.set_criticality(true);
    params.custom_extensions.push(extension);
    let certificate = params
        .self_signed(&key_pair)
        .map_err(|_| McpEndpointTlsError::State)?;
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
        .map_err(|_| McpEndpointTlsError::State)?;
    Ok(Arc::new(CertifiedKey::new(
        vec![CertificateDer::from(certificate.der().to_vec())],
        signing_key,
    )))
}

#[allow(dead_code)] // Called by the same-crate ACME lifecycle owner.
fn der_octet_string(value: &[u8; 32]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(34);
    encoded.extend_from_slice(&[0x04, 0x20]);
    encoded.extend_from_slice(value);
    encoded
}

fn decode_base64_exact(value: &str) -> Result<Vec<u8>, McpEndpointTlsError> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| McpEndpointTlsError::State)?;
    if base64::engine::general_purpose::STANDARD.encode(&decoded) != value {
        return Err(McpEndpointTlsError::State);
    }
    Ok(decoded)
}

fn now_unix_seconds() -> Result<i64, McpEndpointTlsError> {
    Ok(current_epoch_seconds())
}

fn environment_name(environment: McpEndpointCertificateEnvironment) -> &'static str {
    match environment {
        McpEndpointCertificateEnvironment::Staging => "staging",
        McpEndpointCertificateEnvironment::Production => "production",
    }
}

fn is_exact_authorized_hostname(candidate: &str, expected: &str) -> bool {
    !expected.is_empty()
        && candidate == expected
        && candidate.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
        })
}

fn is_acme_attempt(protocols: &[&[u8]]) -> bool {
    protocols.contains(&ACME_TLS_ALPN)
}

fn is_only_acme_alpn(protocols: &[&[u8]]) -> bool {
    protocols == [ACME_TLS_ALPN]
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConnection};
    use solstone_core_journal_io::journal_root::JournalRoot;
    use tempfile::TempDir;

    use super::*;

    const HOSTNAME: &str = "ab12cd34.solstone.me";

    fn fixture_key() -> (Arc<CertifiedKey>, CertificateDer<'static>) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("fixture key");
        let certificate = CertificateParams::new(vec![HOSTNAME.to_owned()])
            .expect("fixture params")
            .self_signed(&key_pair)
            .expect("fixture certificate");
        let cert = CertificateDer::from(certificate.der().to_vec());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
            .expect("fixture signing key");
        (
            Arc::new(CertifiedKey::new(vec![cert.clone()], signing_key)),
            cert,
        )
    }

    fn state_fixture() -> (Vec<Vec<u8>>, Vec<u8>, i64, i64, CertificateDer<'static>) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("state key");
        let now = OffsetDateTime::now_utc();
        let mut params = CertificateParams::new(vec![HOSTNAME.to_owned()]).expect("state params");
        params.not_before = now - Duration::minutes(1);
        params.not_after = now + Duration::hours(1);
        let not_before = params.not_before.unix_timestamp();
        let not_after = params.not_after.unix_timestamp();
        let certificate = params.self_signed(&key_pair).expect("state certificate");
        let der = certificate.der().to_vec();
        (
            vec![der.clone()],
            key_pair.serialize_der(),
            not_before,
            not_after,
            CertificateDer::from(der),
        )
    }

    fn expired_state_fixture() -> (Vec<Vec<u8>>, Vec<u8>, i64, i64) {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("expired state key");
        let now = OffsetDateTime::now_utc();
        let mut params =
            CertificateParams::new(vec![HOSTNAME.to_owned()]).expect("expired state params");
        params.not_before = now - Duration::hours(2);
        params.not_after = now - Duration::hours(1);
        let not_before = params.not_before.unix_timestamp();
        let not_after = params.not_after.unix_timestamp();
        let certificate = params
            .self_signed(&key_pair)
            .expect("expired state certificate");
        (
            vec![certificate.der().to_vec()],
            key_pair.serialize_der(),
            not_before,
            not_after,
        )
    }

    fn trusted_client_config(
        certificate: CertificateDer<'static>,
        alpn: Vec<Vec<u8>>,
    ) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots.add(certificate).expect("fixture root");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("ring provider supports TLS 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = alpn;
        Arc::new(config)
    }

    #[derive(Debug)]
    struct AcmeTestVerifier;

    impl rustls::client::danger::ServerCertVerifier for AcmeTestVerifier {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Err(rustls::Error::General(
                "test client permits TLS 1.3 only".to_owned(),
            ))
        }

        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    fn acme_test_client_config(alpn: Vec<Vec<u8>>) -> Arc<ClientConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("ring provider supports TLS 1.3")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcmeTestVerifier))
            .with_no_client_auth();
        config.alpn_protocols = alpn;
        Arc::new(config)
    }

    fn complete_handshake(
        client: &mut ClientConnection,
        server: &mut ServerConnection,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..16 {
            let mut client_bytes = Vec::new();
            client.write_tls(&mut client_bytes)?;
            if !client_bytes.is_empty() {
                server.read_tls(&mut Cursor::new(client_bytes))?;
                server.process_new_packets()?;
            }
            let mut server_bytes = Vec::new();
            server.write_tls(&mut server_bytes)?;
            if !server_bytes.is_empty() {
                client.read_tls(&mut Cursor::new(server_bytes))?;
                client.process_new_packets()?;
            }
            if !client.is_handshaking() && !server.is_handshaking() {
                return Ok(());
            }
        }
        Err("fixture handshake did not converge".into())
    }

    #[test]
    fn config_is_tls13_and_advertises_the_exact_two_protocols() {
        let service = McpEndpointTlsService::empty_for_test(HOSTNAME.to_owned());
        let config = mcp_endpoint_server_config(&service);
        assert_eq!(
            config.alpn_protocols,
            vec![ACME_TLS_ALPN.to_vec(), HTTP11_ALPN.to_vec()]
        );
    }

    #[test]
    fn reconnect_authority_must_match_the_existing_tls_hostname() {
        let service = McpEndpointTlsService::empty_for_test(HOSTNAME.to_owned());
        assert!(service.matches_authorized_hostname(HOSTNAME));
        assert!(!service.matches_authorized_hostname("other.solstone.me"));
        assert!(!service.matches_authorized_hostname("AB12CD34.solstone.me"));
    }

    #[test]
    fn resolver_never_has_a_default_certificate_without_exact_sni() {
        let service = McpEndpointTlsService::empty_for_test(HOSTNAME.to_owned());
        service.install_ordinary_for_test(fixture_key().0);
        assert!(is_exact_authorized_hostname(HOSTNAME, HOSTNAME));
        assert!(is_exact_authorized_hostname(
            "mcp.my-domain.org",
            "mcp.my-domain.org"
        ));
        assert!(!is_exact_authorized_hostname(
            "AB12CD34.solstone.me",
            HOSTNAME
        ));
        assert!(!is_exact_authorized_hostname("other.solstone.me", HOSTNAME));
        assert!(!is_exact_authorized_hostname(
            "x.ab12cd34.solstone.me",
            HOSTNAME
        ));
    }

    #[test]
    fn acme_attempt_requires_one_exact_protocol_and_an_active_generation() {
        let service = McpEndpointTlsService::empty_for_test(HOSTNAME.to_owned());
        let first = service
            .install_acme_challenge([1; 32])
            .expect("challenge installs");
        let second = service
            .install_acme_challenge([2; 32])
            .expect("replacement installs");
        drop(first);
        assert!(service.resolver.challenge.load_full().is_some());
        drop(second);
        assert!(service.resolver.challenge.load_full().is_none());
        assert!(is_acme_attempt(&[ACME_TLS_ALPN]));
        assert!(is_only_acme_alpn(&[ACME_TLS_ALPN]));
        assert!(!is_only_acme_alpn(&[ACME_TLS_ALPN, HTTP11_ALPN]));
        assert!(!is_only_acme_alpn(&[HTTP11_ALPN, ACME_TLS_ALPN]));
    }

    #[test]
    fn ordinary_install_is_owner_only_durable_and_reloads_before_activation() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, certificate) = state_fixture();
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("empty state service");
        service
            .install_ordinary_certificate(chain, private_key, not_before, not_after)
            .expect("validated state installs");
        assert_eq!(
            fs::metadata(root.path().join("mcp-endpoint/tls"))
                .expect("tls directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join("mcp-endpoint/tls/state.json"))
                .expect("tls state")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let reloaded = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("reloaded valid state");
        let mut client = ClientConnection::new(
            trusted_client_config(certificate, vec![HTTP11_ALPN.to_vec()]),
            ServerName::try_from(HOSTNAME.to_owned()).expect("fixture hostname"),
        )
        .expect("ordinary client");
        let mut server =
            ServerConnection::new(mcp_endpoint_server_config(&reloaded)).expect("reloaded server");
        complete_handshake(&mut client, &mut server).expect("reloaded certificate handshakes");
    }

    #[test]
    fn expired_state_is_not_served_but_is_retained_for_acme_recovery() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after) = expired_state_fixture();
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("empty service");
        let state = StoredCertificateState {
            environment: "staging".to_owned(),
            hostname: HOSTNAME.to_owned(),
            certificate_chain: chain
                .into_iter()
                .map(|value| base64::engine::general_purpose::STANDARD.encode(value))
                .collect(),
            private_key: base64::engine::general_purpose::STANDARD.encode(private_key),
            not_before,
            not_after,
        };
        let encoded = serde_json::to_vec(&state).expect("state JSON");
        let store = service.store.as_ref().expect("state store");
        unix::persist_tls_state_bytes(&store.directory, &encoded).expect("persist expired state");

        let recovered = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("expired state is structurally valid");
        assert!(recovered.resolver.ordinary.load_full().is_none());

        let cache = McpEndpointAcmeCache {
            service: recovered.lifecycle_copy(),
            production: false,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let pem = runtime
            .block_on(CertCache::load_cert(&cache, &[], "staging"))
            .expect("expired certificate cache load")
            .expect("expired certificate remains available to ACME");
        assert!(pem.starts_with(b"-----BEGIN PRIVATE KEY-----"));
    }

    #[test]
    fn forced_staging_renewal_withholds_an_otherwise_current_cached_certificate() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, _) = state_fixture();
        let ordinary = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("empty service");
        ordinary
            .install_ordinary_certificate(chain, private_key, not_before, not_after)
            .expect("current state installs");

        let forced = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            true,
        )
        .expect("forced service opens current state");
        let cache = McpEndpointAcmeCache {
            service: forced.lifecycle_copy(),
            production: false,
            force_staging_renewal: true,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "staging"))
                .expect("forced staging cache load")
                .is_none()
        );
        assert!(
            forced.resolver.ordinary.load_full().is_some(),
            "the last current certificate remains available during reissuance"
        );
        assert!(matches!(
            McpEndpointTlsService::for_authorized_hostname(
                Arc::new(JournalRoot::open(root.path()).expect("production journal root")),
                HOSTNAME.to_owned(),
                McpEndpointCertificateEnvironment::Production,
                true,
            ),
            Err(McpEndpointTlsError::State)
        ));
    }

    #[test]
    fn acme_account_state_is_environment_scoped_and_owner_only() {
        let root = TempDir::new().expect("state root");
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("empty service");
        let account =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .expect("account key");
        let store = service.store.as_ref().expect("state store");
        unix::persist_tls_acme_account_bytes(&store.directory, false, account.as_ref())
            .expect("persist staging account");
        assert!(
            unix::read_tls_acme_account_bytes(&store.directory, true)
                .expect("read production account")
                .is_none()
        );
        assert_eq!(
            fs::metadata(root.path().join("mcp-endpoint/tls/account-staging.pk8"))
                .expect("staging account")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn renewal_honors_a_preexisting_shutdown_without_acme_work() {
        let root = TempDir::new().expect("state root");
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("empty service");
        let (_sender, mut shutdown) = tokio::sync::watch::channel(true);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(service.run_acme_renewal(&mut shutdown))
            .expect("preexisting shutdown is clean");
        assert!(
            !root
                .path()
                .join("mcp-endpoint/tls/account-staging.pk8")
                .exists()
        );
        assert!(!root.path().join("mcp-endpoint/tls/state.json").exists());
    }

    #[test]
    fn state_parser_rejects_duplicate_unknown_and_environment_mismatch() {
        let duplicate = br#"{
            "environment":"staging","environment":"staging",
            "hostname":"ab12cd34.solstone.me","certificate_chain":[],
            "private_key":"","not_before":1,"not_after":2
        }"#;
        assert!(matches!(
            validate_stored_state(
                duplicate,
                HOSTNAME,
                McpEndpointCertificateEnvironment::Staging
            ),
            Err(McpEndpointTlsError::State)
        ));
        let unknown = br#"{
            "environment":"staging","hostname":"ab12cd34.solstone.me",
            "certificate_chain":[],"private_key":"","not_before":1,
            "not_after":2,"other":true
        }"#;
        assert!(matches!(
            validate_stored_state(
                unknown,
                HOSTNAME,
                McpEndpointCertificateEnvironment::Staging
            ),
            Err(McpEndpointTlsError::State)
        ));
    }

    #[test]
    fn staging_state_is_foreign_under_production() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, _) = state_fixture();
        let staging_service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("staging root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("staging service");
        staging_service
            .install_ordinary_certificate(chain, private_key, not_before, not_after)
            .expect("install staging state");

        let state_path = root.path().join("mcp-endpoint/tls/state.json");
        let before_bytes = fs::read(&state_path).expect("read staging state");

        let prod_service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("production root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("production service opens foreign staging state cleanly");
        assert!(
            prod_service.resolver.ordinary.load_full().is_none(),
            "foreign staging state must not be served under production"
        );

        let cache = McpEndpointAcmeCache {
            service: prod_service.lifecycle_copy(),
            production: true,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "production"))
                .expect("foreign state load_cert is a clean miss, not an error")
                .is_none()
        );

        let after_bytes = fs::read(&state_path).expect("read state after prod open");
        assert_eq!(
            before_bytes, after_bytes,
            "foreign state file must not be rewritten on open or miss"
        );
    }

    #[test]
    fn production_install_activates_and_reuses_under_production() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, certificate) = state_fixture();
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("empty production service");
        service
            .install_ordinary_certificate(chain, private_key, not_before, not_after)
            .expect("validated production state installs");

        let reloaded = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("reloaded valid production state");
        assert!(reloaded.resolver.ordinary.load_full().is_some());

        let mut client = ClientConnection::new(
            trusted_client_config(certificate, vec![HTTP11_ALPN.to_vec()]),
            ServerName::try_from(HOSTNAME.to_owned()).expect("fixture hostname"),
        )
        .expect("ordinary client");
        let mut server =
            ServerConnection::new(mcp_endpoint_server_config(&reloaded)).expect("reloaded server");
        complete_handshake(&mut client, &mut server)
            .expect("reloaded production certificate handshakes");

        let second_reload = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("second reopen")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("second production reopen");
        assert!(second_reload.resolver.ordinary.load_full().is_some());

        let cache = McpEndpointAcmeCache {
            service: second_reload.lifecycle_copy(),
            production: true,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let pem = runtime
            .block_on(CertCache::load_cert(&cache, &[], "production"))
            .expect("load production cert")
            .expect("production cert is available to ACME");
        assert!(pem.starts_with(b"-----BEGIN PRIVATE KEY-----"));
    }

    #[test]
    fn production_state_is_foreign_under_explicit_staging() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, _) = state_fixture();
        let prod_service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("prod root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("prod service");
        prod_service
            .install_ordinary_certificate(chain, private_key, not_before, not_after)
            .expect("install production state");

        let state_path = root.path().join("mcp-endpoint/tls/state.json");
        let before_bytes = fs::read(&state_path).expect("read production state");

        let staging_service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("staging root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("staging service opens foreign production state cleanly");
        assert!(
            staging_service.resolver.ordinary.load_full().is_none(),
            "foreign production state must not be served under staging"
        );

        let cache = McpEndpointAcmeCache {
            service: staging_service.lifecycle_copy(),
            production: false,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "staging"))
                .expect("foreign state load_cert is a clean miss, not an error")
                .is_none()
        );

        let after_bytes = fs::read(&state_path).expect("read state after staging open");
        assert_eq!(before_bytes, after_bytes);
    }

    #[test]
    fn corrupt_state_is_integrity_failure_under_production() {
        let root = TempDir::new().expect("state root");
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("empty service");
        let store = service.store.as_ref().expect("state store");
        unix::persist_tls_state_bytes(&store.directory, b"{\"corrupt\": true}")
            .expect("persist corrupt state");

        assert!(matches!(
            McpEndpointTlsService::for_authorized_hostname(
                Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
                HOSTNAME.to_owned(),
                McpEndpointCertificateEnvironment::Production,
                false,
            ),
            Err(McpEndpointTlsError::State)
        ));

        let cache = McpEndpointAcmeCache {
            service: service.lifecycle_copy(),
            production: true,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "production"))
                .is_err(),
            "corrupt state must fail load_cert, never return miss"
        );
    }

    #[test]
    fn hostname_mismatched_state_is_integrity_failure_under_production() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, _) = state_fixture();
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("empty service");
        let other_hostname = "cd56ef78.solstone.me";
        let state = StoredCertificateState {
            environment: "production".to_owned(),
            hostname: other_hostname.to_owned(),
            certificate_chain: chain
                .into_iter()
                .map(|value| base64::engine::general_purpose::STANDARD.encode(value))
                .collect(),
            private_key: base64::engine::general_purpose::STANDARD.encode(private_key),
            not_before,
            not_after,
        };
        let encoded = serde_json::to_vec(&state).expect("state JSON");
        let store = service.store.as_ref().expect("state store");
        unix::persist_tls_state_bytes(&store.directory, &encoded)
            .expect("persist mismatched state");

        assert!(matches!(
            McpEndpointTlsService::for_authorized_hostname(
                Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
                HOSTNAME.to_owned(),
                McpEndpointCertificateEnvironment::Production,
                false,
            ),
            Err(McpEndpointTlsError::State)
        ));

        let cache = McpEndpointAcmeCache {
            service: service.lifecycle_copy(),
            production: true,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "production"))
                .is_err(),
            "hostname mismatch must fail load_cert, never return miss"
        );
    }

    #[test]
    fn tls_permissions_failure_is_integrity_failure_under_production() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, _) = state_fixture();
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("service");
        service
            .install_ordinary_certificate(chain, private_key, not_before, not_after)
            .expect("install state");

        let state_path = root.path().join("mcp-endpoint/tls/state.json");
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o644))
            .expect("chmod state.json to invalid 0o644");

        assert!(matches!(
            McpEndpointTlsService::for_authorized_hostname(
                Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
                HOSTNAME.to_owned(),
                McpEndpointCertificateEnvironment::Production,
                false,
            ),
            Err(McpEndpointTlsError::State)
        ));

        let cache = McpEndpointAcmeCache {
            service: service.lifecycle_copy(),
            production: true,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "production"))
                .is_err(),
            "invalid permissions must fail load_cert, never return miss"
        );
    }

    #[test]
    fn expired_foreign_state_is_not_served_and_yields_cache_miss_under_production() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after) = expired_state_fixture();
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .expect("empty service");
        let state = StoredCertificateState {
            environment: "staging".to_owned(),
            hostname: HOSTNAME.to_owned(),
            certificate_chain: chain
                .into_iter()
                .map(|value| base64::engine::general_purpose::STANDARD.encode(value))
                .collect(),
            private_key: base64::engine::general_purpose::STANDARD.encode(private_key),
            not_before,
            not_after,
        };
        let encoded = serde_json::to_vec(&state).expect("state JSON");
        let store = service.store.as_ref().expect("state store");
        unix::persist_tls_state_bytes(&store.directory, &encoded)
            .expect("persist expired staging state");

        let prod_service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("expired foreign state is structurally valid and opens cleanly");
        assert!(prod_service.resolver.ordinary.load_full().is_none());

        let cache = McpEndpointAcmeCache {
            service: prod_service.lifecycle_copy(),
            production: true,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "production"))
                .expect("expired foreign cert load_cert")
                .is_none(),
            "expired foreign cert is a cache miss and must not be returned to ACME"
        );
    }

    #[test]
    fn unknown_environment_string_is_integrity_failure() {
        let root = TempDir::new().expect("state root");
        let (chain, private_key, not_before, not_after, _) = state_fixture();
        let service = McpEndpointTlsService::for_authorized_hostname(
            Arc::new(JournalRoot::open(root.path()).expect("journal root")),
            HOSTNAME.to_owned(),
            McpEndpointCertificateEnvironment::Production,
            false,
        )
        .expect("empty service");
        let state = StoredCertificateState {
            environment: "test".to_owned(),
            hostname: HOSTNAME.to_owned(),
            certificate_chain: chain
                .into_iter()
                .map(|value| base64::engine::general_purpose::STANDARD.encode(value))
                .collect(),
            private_key: base64::engine::general_purpose::STANDARD.encode(private_key),
            not_before,
            not_after,
        };
        let encoded = serde_json::to_vec(&state).expect("state JSON");
        let store = service.store.as_ref().expect("state store");
        unix::persist_tls_state_bytes(&store.directory, &encoded)
            .expect("persist unknown env state");

        assert!(matches!(
            McpEndpointTlsService::for_authorized_hostname(
                Arc::new(JournalRoot::open(root.path()).expect("reopened journal root")),
                HOSTNAME.to_owned(),
                McpEndpointCertificateEnvironment::Production,
                false,
            ),
            Err(McpEndpointTlsError::State)
        ));

        let cache = McpEndpointAcmeCache {
            service: service.lifecycle_copy(),
            production: true,
            force_staging_renewal: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert!(
            runtime
                .block_on(CertCache::load_cert(&cache, &[], "production"))
                .is_err(),
            "unknown environment string must fail load_cert"
        );
    }

    #[test]
    fn challenge_certificate_has_exact_rfc8737_identifier_and_only_negotiates_acme() {
        let service = McpEndpointTlsService::empty_for_test(HOSTNAME.to_owned());
        let digest = [0x5a; 32];
        let guard = service
            .install_acme_challenge(digest)
            .expect("challenge installs");
        let challenge = service
            .resolver
            .challenge
            .load_full()
            .expect("challenge active");
        let certificate = challenge.key.cert.first().expect("challenge leaf").clone();
        let (_, parsed) = parse_x509_certificate(certificate.as_ref()).expect("parse challenge");
        let names = parsed
            .subject_alternative_name()
            .expect("parse SAN")
            .expect("challenge SAN");
        assert_eq!(names.value.general_names.len(), 1);
        assert!(matches!(
            names.value.general_names.first(),
            Some(GeneralName::DNSName(name)) if *name == HOSTNAME
        ));
        let identifier = parsed
            .extensions()
            .iter()
            .find(|extension| extension.oid.to_id_string() == "1.3.6.1.5.5.7.1.31")
            .expect("acme identifier extension");
        assert!(identifier.critical);
        assert_eq!(identifier.value, der_octet_string(&digest));

        let mut client = ClientConnection::new(
            acme_test_client_config(vec![ACME_TLS_ALPN.to_vec()]),
            ServerName::try_from(HOSTNAME.to_owned()).expect("acme hostname"),
        )
        .expect("acme client");
        let mut server =
            ServerConnection::new(mcp_endpoint_server_config(&service)).expect("acme server");
        complete_handshake(&mut client, &mut server).expect("acme handshake");
        assert_eq!(client.alpn_protocol(), Some(ACME_TLS_ALPN));
        drop(guard);
    }

    #[test]
    fn ordinary_http11_handshake_succeeds_but_h2_only_fails() {
        let service = McpEndpointTlsService::empty_for_test(HOSTNAME.to_owned());
        let (key, certificate) = fixture_key();
        service.install_ordinary_for_test(key);
        let server_config = mcp_endpoint_server_config(&service);

        let mut ordinary_client = ClientConnection::new(
            trusted_client_config(certificate.clone(), vec![HTTP11_ALPN.to_vec()]),
            ServerName::try_from(HOSTNAME.to_owned()).expect("fixture hostname"),
        )
        .expect("ordinary client");
        let mut ordinary_server =
            ServerConnection::new(Arc::clone(&server_config)).expect("ordinary server");
        complete_handshake(&mut ordinary_client, &mut ordinary_server).expect("http1 handshake");
        assert_eq!(ordinary_client.alpn_protocol(), Some(HTTP11_ALPN));

        let mut h2_client = ClientConnection::new(
            trusted_client_config(certificate, vec![b"h2".to_vec()]),
            ServerName::try_from(HOSTNAME.to_owned()).expect("h2 hostname"),
        )
        .expect("h2 client");
        let mut h2_server = ServerConnection::new(server_config).expect("h2 server");
        assert!(complete_handshake(&mut h2_client, &mut h2_server).is_err());
    }

    #[test]
    fn hot_swap_changes_the_per_handshake_key_without_rebuilding_the_resolver() {
        let service = McpEndpointTlsService::empty_for_test(HOSTNAME.to_owned());
        service.install_ordinary_for_test(fixture_key().0);
        let first = service
            .resolver
            .ordinary
            .load_full()
            .expect("first active ordinary key");
        service.install_ordinary_for_test(fixture_key().0);
        let second = service
            .resolver
            .ordinary
            .load_full()
            .expect("second active ordinary key");
        assert!(!Arc::ptr_eq(&first, &second));
    }
}
