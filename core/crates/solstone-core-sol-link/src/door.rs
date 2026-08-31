// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal device-door TLS authorization.
//!
//! Design decisions:
//! - This lives beside, rather than inside, the ledger because it owns the TLS boundary.
//! - [`DeviceDoorAuthorization`] publishes the complete ledger read posture, never a flattened snapshot.
//! - One ledger-owning Tokio task refreshes that posture over `watch` on a caller-selected interval.
//! - Direct verifier tests live here; duplex TLS tests live in `tests/device_door_tls.rs`.
//! - [`build_device_door_server_config`] returns an `Arc<ServerConfig>` for direct use by Tokio TLS.
//!
//! [check] `read_authorized` treats a malformed individual array entry as a malformed ledger.
//! This module publishes that ledger-owned posture without attempting to reinterpret it.

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error as RustlsError, OtherError,
    RootCertStore, ServerConfig, SignatureScheme,
};
use tokio::sync::watch;

use crate::ledger::{AuthorizationLedger, AuthorizedClientsRead};

static AUTHORIZATION_PUBLICATION_TICKS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Cumulative count of refresh-loop ticks that have fired, process-wide.
/// Debug builds only advance it; assert deltas, never an absolute value —
/// it accumulates across every test in a `--test-threads=1` binary and is
/// shared by every live `spawn_authorization_refresh` publisher.
pub fn authorization_publication_ticks() -> u64 {
    AUTHORIZATION_PUBLICATION_TICKS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The complete, read-only authorization posture published to the device door.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceDoorAuthorization(AuthorizedClientsRead);

impl DeviceDoorAuthorization {
    pub fn as_read(&self) -> &AuthorizedClientsRead {
        &self.0
    }
}

impl From<AuthorizedClientsRead> for DeviceDoorAuthorization {
    fn from(value: AuthorizedClientsRead) -> Self {
        Self(value)
    }
}

/// Refresh a device-door authorization publication from its single ledger owner.
pub fn refresh_once(
    ledger: &mut AuthorizationLedger,
    sender: &watch::Sender<DeviceDoorAuthorization>,
) {
    ledger.reload_if_stale();
    sender.send_replace(DeviceDoorAuthorization::from(ledger.read_state()));
}

/// Spawn the ledger-owning refresh loop for a caller-owned authorization publication.
pub fn spawn_authorization_refresh(
    mut ledger: AuthorizationLedger,
    sender: watch::Sender<DeviceDoorAuthorization>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    assert!(
        interval > Duration::ZERO,
        "authorization refresh interval must be greater than zero"
    );
    refresh_once(&mut ledger, &sender);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            #[cfg(debug_assertions)]
            AUTHORIZATION_PUBLICATION_TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            refresh_once(&mut ledger, &sender);
        }
    })
}

/// TLS client-certificate verifier for the journal's paired-device door.
///
/// ```compile_fail,E0308
/// use std::path::PathBuf;
/// use solstone_core_sol_link::DeviceDoorVerifier;
/// use solstone_core_sol_link::ledger::AuthorizationLedger;
///
/// fn cannot_construct_with_storage(path: PathBuf, ledger: AuthorizationLedger) {
///     let _ = DeviceDoorVerifier::new(path, ledger);
/// }
/// ```
pub struct DeviceDoorVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    authorization: DeviceDoorAuthorization,
}

impl fmt::Debug for DeviceDoorVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceDoorVerifier")
            .finish_non_exhaustive()
    }
}

impl DeviceDoorVerifier {
    pub fn new(inner: Arc<dyn ClientCertVerifier>, authorization: DeviceDoorAuthorization) -> Self {
        Self {
            inner,
            authorization,
        }
    }
}

impl ClientCertVerifier for DeviceDoorVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, RustlsError> {
        self.inner
            .verify_client_cert(end_entity, intermediates, now)?;

        let lookup_key = certificate_lookup_key(end_entity);
        match self.authorization.as_read() {
            AuthorizedClientsRead::Present(entries)
                if entries.iter().any(|entry| entry.fingerprint == lookup_key) =>
            {
                Ok(ClientCertVerified::assertion())
            }
            AuthorizedClientsRead::Present(_) | AuthorizedClientsRead::Missing => Err(
                RustlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure),
            ),
            AuthorizedClientsRead::Unreadable | AuthorizedClientsRead::Malformed => {
                Err(authorization_unavailable_error())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        self.inner.requires_raw_public_keys()
    }
}

/// Errors while building a device-door server TLS configuration.
#[derive(Debug)]
pub enum DeviceDoorConfigError {
    Rustls(RustlsError),
    Verifier(rustls::server::VerifierBuilderError),
}

impl fmt::Display for DeviceDoorConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rustls(error) => error.fmt(formatter),
            Self::Verifier(error) => error.fmt(formatter),
        }
    }
}

impl Error for DeviceDoorConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Rustls(error) => Some(error),
            Self::Verifier(error) => Some(error),
        }
    }
}

/// Build a ring-backed server configuration that requires a paired client certificate.
pub fn build_device_door_server_config(
    server_cert_chain: Vec<CertificateDer<'static>>,
    server_key: PrivateKeyDer<'static>,
    client_ca: CertificateDer<'static>,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
) -> Result<Arc<ServerConfig>, DeviceDoorConfigError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots
        .add(client_ca)
        .map_err(DeviceDoorConfigError::Rustls)?;
    let inner = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .map_err(DeviceDoorConfigError::Verifier)?;
    let verifier = Arc::new(DeviceDoorVerifier::new(
        inner,
        authorization.borrow().clone(),
    ));
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(DeviceDoorConfigError::Rustls)?
        .with_client_cert_verifier(verifier)
        .with_single_cert(server_cert_chain, server_key)
        .map_err(DeviceDoorConfigError::Rustls)?;
    Ok(Arc::new(config))
}

fn certificate_lookup_key(certificate: &CertificateDer<'_>) -> String {
    format!("sha256:{}", spl_core::ca::sha256_hex(certificate.as_ref()))
}

fn authorization_unavailable_error() -> RustlsError {
    let source: Arc<dyn Error + Send + Sync> = Arc::new(io::Error::other(
        "paired devices authorization ledger is unreadable",
    ));
    RustlsError::InvalidCertificate(CertificateError::Other(OtherError(source)))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    };
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use rustls::{AlertDescription, ClientConfig, SignatureScheme, SupportedProtocolVersion};
    use tokio_rustls::{TlsAcceptor, TlsConnector};
    use x509_parser::pem::parse_x509_pem;

    use super::*;
    use crate::{
        ledger::{ClientEntry, ClientRole},
        test_support::{FIXED_CERTIFICATE_PEM, FIXED_CERTIFICATE_SHA256},
    };

    #[test]
    fn lookup_key_uses_fixed_leaf_der_digest() {
        let (_, pem) = parse_x509_pem(FIXED_CERTIFICATE_PEM.as_bytes()).unwrap();
        let certificate = CertificateDer::from(pem.contents);
        assert_eq!(
            certificate_lookup_key(&certificate),
            format!("sha256:{FIXED_CERTIFICATE_SHA256}")
        );
    }

    #[test]
    fn tls13_signature_delegates_for_matching_and_mismatched_keys() {
        let correct = capture_signature(&rustls::version::TLS13);
        let wrong = capture_signature(&rustls::version::TLS13);
        assert!(
            correct
                .verifier
                .verify_tls13_signature(&correct.message, &correct.certificate, &correct.signature)
                .is_ok()
        );
        assert!(
            correct
                .verifier
                .verify_tls13_signature(&wrong.message, &correct.certificate, &wrong.signature)
                .is_err()
        );
    }

    #[test]
    fn tls12_signature_delegates_for_matching_and_mismatched_keys() {
        let correct = capture_signature(&rustls::version::TLS12);
        let wrong = capture_signature(&rustls::version::TLS12);
        assert!(
            correct
                .verifier
                .verify_tls12_signature(&correct.message, &correct.certificate, &correct.signature)
                .is_ok()
        );
        assert!(
            correct
                .verifier
                .verify_tls12_signature(&wrong.message, &correct.certificate, &wrong.signature)
                .is_err()
        );
    }

    #[test]
    fn supported_verify_schemes_match_inner_verifier() {
        let fixture = Fixture::new();
        assert_eq!(
            fixture
                .verifier_for(AuthorizedClientsRead::Missing)
                .supported_verify_schemes(),
            fixture.inner.supported_verify_schemes()
        );
    }

    #[test]
    fn matching_present_entry_is_accepted() {
        let fixture = Fixture::new();
        assert!(
            fixture
                .verify(AuthorizedClientsRead::Present(vec![entry(
                    &fixture.client_cert
                )]))
                .is_ok()
        );
    }

    #[test]
    fn absent_present_entry_and_missing_are_access_denied() {
        let fixture = Fixture::new();
        assert_access_denied(fixture.verify(AuthorizedClientsRead::Present(Vec::new())));
        assert_access_denied(fixture.verify(AuthorizedClientsRead::Missing));
    }

    #[test]
    fn unreadable_ledger_is_certificate_unknown() {
        let fixture = Fixture::new();
        let temporary = TempDir::new();
        let path = temporary.path().join("authorized_clients.json");
        fs::create_dir(&path).unwrap();
        let mut ledger =
            AuthorizationLedger::from_paths(path, temporary.path().join("devices.json"));
        let state = ledger.read_state();
        assert_eq!(state, AuthorizedClientsRead::Unreadable);
        assert_certificate_unknown(fixture.verify(state));
    }

    #[test]
    fn malformed_ledger_is_certificate_unknown() {
        let fixture = Fixture::new();
        let temporary = TempDir::new();
        let path = temporary.path().join("authorized_clients.json");
        fs::write(&path, b"{}").unwrap();
        let mut ledger =
            AuthorizationLedger::from_paths(path, temporary.path().join("devices.json"));
        let state = ledger.read_state();
        assert_eq!(state, AuthorizedClientsRead::Malformed);
        assert_certificate_unknown(fixture.verify(state));
    }

    #[test]
    fn unrelated_ca_is_rejected_before_authorization_lookup() {
        let fixture = Fixture::new();
        let unrelated = Fixture::new();
        let result = fixture
            .verifier_for(AuthorizedClientsRead::Present(vec![entry(
                &unrelated.client_cert,
            )]))
            .verify_client_cert(&unrelated.client_cert, &[], UnixTime::now());
        assert!(result.is_err());
    }

    struct Fixture {
        inner: Arc<dyn ClientCertVerifier>,
        client_cert: CertificateDer<'static>,
    }

    impl Fixture {
        fn new() -> Self {
            let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut ca_params = CertificateParams::default();
            ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            ca_params.key_usages = vec![
                KeyUsagePurpose::DigitalSignature,
                KeyUsagePurpose::KeyCertSign,
            ];
            let ca = ca_params.self_signed(&ca_key).unwrap();
            let client_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut client_params = CertificateParams::default();
            client_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
            let client = client_params.signed_by(&client_key, &ca, &ca_key).unwrap();
            let client_cert = CertificateDer::from(client.der().to_vec());
            let inner = inner_for(CertificateDer::from(ca.der().to_vec()));
            Self { inner, client_cert }
        }

        fn verifier_for(&self, state: AuthorizedClientsRead) -> DeviceDoorVerifier {
            DeviceDoorVerifier::new(self.inner.clone(), DeviceDoorAuthorization::from(state))
        }

        fn verify(&self, state: AuthorizedClientsRead) -> Result<ClientCertVerified, RustlsError> {
            self.verifier_for(state)
                .verify_client_cert(&self.client_cert, &[], UnixTime::now())
        }
    }

    fn inner_for(ca: CertificateDer<'static>) -> Arc<dyn ClientCertVerifier> {
        let mut roots = RootCertStore::empty();
        roots.add(ca).unwrap();
        rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap()
    }

    fn entry(certificate: &CertificateDer<'_>) -> ClientEntry {
        ClientEntry::new(
            certificate_lookup_key(certificate),
            "phone",
            "2026-01-01T00:00:00Z",
            "instance",
            ClientRole::Roleless,
        )
    }

    fn assert_access_denied(result: Result<ClientCertVerified, RustlsError>) {
        assert!(matches!(
            result,
            Err(RustlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure
            ))
        ));
        assert_eq!(
            AlertDescription::from(CertificateError::ApplicationVerificationFailure),
            AlertDescription::AccessDenied
        );
    }

    fn assert_certificate_unknown(result: Result<ClientCertVerified, RustlsError>) {
        assert!(matches!(
            result,
            Err(RustlsError::InvalidCertificate(CertificateError::Other(_)))
        ));
        let source: Arc<dyn Error + Send + Sync> = Arc::new(io::Error::other("test"));
        assert_eq!(
            AlertDescription::from(CertificateError::Other(OtherError(source))),
            AlertDescription::CertificateUnknown
        );
    }

    #[derive(Debug, Clone)]
    struct RecordedSignature {
        message: Vec<u8>,
        certificate: CertificateDer<'static>,
        signature: DigitallySignedStruct,
    }

    #[derive(Debug)]
    struct SignatureRecorder {
        inner: Arc<dyn ClientCertVerifier>,
        recorded: Mutex<Option<RecordedSignature>>,
    }

    impl SignatureRecorder {
        fn take(&self) -> RecordedSignature {
            self.recorded.lock().unwrap().take().unwrap()
        }
    }

    impl ClientCertVerifier for SignatureRecorder {
        fn offer_client_auth(&self) -> bool {
            self.inner.offer_client_auth()
        }

        fn client_auth_mandatory(&self) -> bool {
            self.inner.client_auth_mandatory()
        }

        fn root_hint_subjects(&self) -> &[DistinguishedName] {
            self.inner.root_hint_subjects()
        }

        fn verify_client_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            intermediates: &[CertificateDer<'_>],
            now: UnixTime,
        ) -> Result<ClientCertVerified, RustlsError> {
            self.inner
                .verify_client_cert(end_entity, intermediates, now)
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            certificate: &CertificateDer<'_>,
            signature: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, RustlsError> {
            *self.recorded.lock().unwrap() = Some(RecordedSignature {
                message: message.to_vec(),
                certificate: certificate.clone().into_owned(),
                signature: signature.clone(),
            });
            self.inner
                .verify_tls12_signature(message, certificate, signature)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            certificate: &CertificateDer<'_>,
            signature: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, RustlsError> {
            *self.recorded.lock().unwrap() = Some(RecordedSignature {
                message: message.to_vec(),
                certificate: certificate.clone().into_owned(),
                signature: signature.clone(),
            });
            self.inner
                .verify_tls13_signature(message, certificate, signature)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.inner.supported_verify_schemes()
        }

        fn requires_raw_public_keys(&self) -> bool {
            self.inner.requires_raw_public_keys()
        }
    }

    struct CapturedSignature {
        verifier: Arc<DeviceDoorVerifier>,
        message: Vec<u8>,
        certificate: CertificateDer<'static>,
        signature: DigitallySignedStruct,
    }

    fn capture_signature(version: &'static SupportedProtocolVersion) -> CapturedSignature {
        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
        ];
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let (server_certificate, server_key) =
            issued_identity(&ca, &ca_key, ExtendedKeyUsagePurpose::ServerAuth);
        let (client_certificate, client_key) =
            issued_identity(&ca, &ca_key, ExtendedKeyUsagePurpose::ClientAuth);
        let inner = inner_for(CertificateDer::from(ca.der().to_vec()));
        let recorder = Arc::new(SignatureRecorder {
            inner,
            recorded: Mutex::new(None),
        });
        let verifier = Arc::new(DeviceDoorVerifier::new(
            recorder.clone(),
            DeviceDoorAuthorization::from(AuthorizedClientsRead::Present(vec![entry(
                &client_certificate,
            )])),
        ));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_config = Arc::new(
            ServerConfig::builder_with_provider(provider.clone())
                .with_protocol_versions(&[version])
                .unwrap()
                .with_client_cert_verifier(verifier.clone())
                .with_single_cert(vec![server_certificate], server_key)
                .unwrap(),
        );
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(ca.der().to_vec())).unwrap();
        let client_config = Arc::new(
            ClientConfig::builder_with_provider(provider)
                .with_protocol_versions(&[version])
                .unwrap()
                .with_root_certificates(roots)
                .with_client_auth_cert(vec![client_certificate], client_key)
                .unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (server_stream, client_stream) = tokio::io::duplex(32 * 1024);
            let (server, client) = tokio::join!(
                TlsAcceptor::from(server_config).accept(server_stream),
                TlsConnector::from(client_config)
                    .connect(ServerName::try_from("door.test").unwrap(), client_stream),
            );
            assert!(server.is_ok());
            assert!(client.is_ok());
        });
        let signature = recorder.take();
        CapturedSignature {
            verifier,
            message: signature.message,
            certificate: signature.certificate,
            signature: signature.signature,
        }
    }

    fn issued_identity(
        ca: &rcgen::Certificate,
        ca_key: &KeyPair,
        usage: ExtendedKeyUsagePurpose,
    ) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::new(vec!["door.test".to_string()]).unwrap();
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![usage];
        let certificate = params.signed_by(&key, ca, ca_key).unwrap();
        (
            CertificateDer::from(certificate.der().to_vec()),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sol-link-door-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
