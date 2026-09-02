// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{
    CertificateParams, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{AlertDescription, ClientConfig, Error as RustlsError, RootCertStore};
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, ClientEntry, ClientRole,
};
use solstone_core_sol_link::test_support::TestCa;
use solstone_core_sol_link::{
    DeviceDoorAuthorization, build_device_door_server_config, refresh_once,
};
use tokio::io::{AsyncReadExt, DuplexStream};
use tokio::sync::watch;
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[tokio::test]
async fn authorized_fingerprint_completes_the_handshake() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    ledger.add(fixture.entry()).unwrap();
    let (sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver);

    let (server, client) = handshake(config, fixture.client_config()).await;

    assert!(server.is_ok());
    assert!(client.is_ok());
    drop(sender);
}

#[tokio::test]
async fn absent_fingerprint_is_refused_with_access_denied() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let unrelated = TlsFixture::new();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    ledger.add(unrelated.entry()).unwrap();
    let (_sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver);

    let (server, client) = handshake(config, fixture.client_config()).await;

    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::AccessDenied);
}

#[tokio::test]
async fn missing_ledger_is_refused_with_access_denied() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    let (_sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver);

    let (server, client) = handshake(config, fixture.client_config()).await;

    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::AccessDenied);
}

#[tokio::test]
async fn malformed_ledger_is_refused_with_certificate_unknown() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let path = temporary
        .path()
        .join("link")
        .join("authorized_clients.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{}").unwrap();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    let (_sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver);

    let (server, client) = handshake(config, fixture.client_config()).await;

    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::CertificateUnknown);
}

#[tokio::test]
async fn duplicate_cid_ledger_is_refused_with_certificate_unknown() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let path = temporary
        .path()
        .join("link")
        .join("authorized_clients.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        br#"[{"fingerprint":"a","device_label":"one","paired_at":"1","instance_id":"i"},{"fingerprint":"a","device_label":"two","paired_at":"2","instance_id":"i"}]"#,
    )
    .unwrap();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    let (_sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver);

    let (server, client) = handshake(config, fixture.client_config()).await;

    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::CertificateUnknown);
}

#[tokio::test]
async fn unreadable_ledger_is_refused_with_certificate_unknown() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let path = temporary
        .path()
        .join("link")
        .join("authorized_clients.json");
    fs::create_dir_all(&path).unwrap();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    let (_sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver);

    let (server, client) = handshake(config, fixture.client_config()).await;

    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::CertificateUnknown);
}

#[tokio::test]
async fn server_config_built_after_refresh_snapshots_revoked_posture() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    let entry = fixture.entry();
    ledger.add(entry.clone()).unwrap();
    let (sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver.clone());

    let (server, client) = handshake(config.clone(), fixture.client_config()).await;
    assert!(server.is_ok());
    assert!(client.is_ok());

    assert!(
        ledger
            .remove(&entry.fingerprint)
            .unwrap()
            .authorized_removed
    );
    refresh_once(&mut ledger, &sender);
    let refreshed_config = fixture.server_config(receiver);
    let (server, client) = handshake(refreshed_config, fixture.client_config()).await;
    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::AccessDenied);
}

#[tokio::test]
async fn server_config_built_after_refresh_snapshots_unreadable_posture() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    ledger.add(fixture.entry()).unwrap();
    let (sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver.clone());

    let (server, client) = handshake(config.clone(), fixture.client_config()).await;
    assert!(server.is_ok());
    assert!(client.is_ok());

    let path = ledger.authorized_clients_path().to_path_buf();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    refresh_once(&mut ledger, &sender);
    let refreshed_config = fixture.server_config(receiver);
    let (server, client) = handshake(refreshed_config, fixture.client_config()).await;
    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::CertificateUnknown);
}

#[tokio::test]
async fn server_config_built_after_refresh_snapshots_duplicate_cid_posture() {
    let temporary = TempDir::new();
    let fixture = TlsFixture::new();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    ledger.add(fixture.entry()).unwrap();
    let (sender, receiver) = authorization_channel(&mut ledger);
    let config = fixture.server_config(receiver.clone());

    let (server, client) = handshake(config.clone(), fixture.client_config()).await;
    assert!(server.is_ok());
    assert!(client.is_ok());

    let path = ledger.authorized_clients_path().to_path_buf();
    fs::write(
        &path,
        br#"[{"fingerprint":"a","device_label":"one","paired_at":"1","instance_id":"i"},{"fingerprint":"a","device_label":"two","paired_at":"2","instance_id":"i"}]"#,
    )
    .unwrap();
    refresh_once(&mut ledger, &sender);
    let refreshed_config = fixture.server_config(receiver);
    let (server, client) = handshake(refreshed_config, fixture.client_config()).await;
    assert!(server.is_err());
    assert_peer_alert(client, AlertDescription::CertificateUnknown);
}

struct TlsFixture {
    ca: TestCa,
    server_cert: CertificateDer<'static>,
    server_key: PrivateKeyDer<'static>,
    client_cert: CertificateDer<'static>,
    client_key: PrivateKeyDer<'static>,
}

impl TlsFixture {
    fn new() -> Self {
        let ca = TestCa::new();
        let (server_cert, server_key) = issued_identity(&ca, ExtendedKeyUsagePurpose::ServerAuth);
        let (client_cert, client_key) = issued_identity(&ca, ExtendedKeyUsagePurpose::ClientAuth);
        Self {
            ca,
            server_cert,
            server_key,
            client_cert,
            client_key,
        }
    }

    fn entry(&self) -> ClientEntry {
        ClientEntry::new(
            fingerprint(&self.client_cert),
            "phone",
            "2026-01-01T00:00:00Z",
            "instance",
            ClientRole::Roleless,
        )
    }

    fn server_config(
        &self,
        authorization: watch::Receiver<DeviceDoorAuthorization>,
    ) -> Arc<rustls::ServerConfig> {
        build_device_door_server_config(
            vec![self.server_cert.clone()],
            self.server_key.clone_key(),
            CertificateDer::from(self.ca.certificate().der().to_vec()),
            authorization,
        )
        .unwrap()
    }

    fn client_config(&self) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(self.ca.certificate().der().to_vec()))
            .unwrap();
        Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_client_auth_cert(vec![self.client_cert.clone()], self.client_key.clone_key())
                .unwrap(),
        )
    }
}

fn issued_identity(
    ca: &TestCa,
    usage: ExtendedKeyUsagePurpose,
) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::new(vec!["door.test".to_string()]).unwrap();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![usage];
    let certificate = params.signed_by(&key, ca.certificate(), ca.key()).unwrap();
    (
        CertificateDer::from(certificate.der().to_vec()),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
}

fn authorization_channel(
    ledger: &mut AuthorizationLedger,
) -> (
    watch::Sender<DeviceDoorAuthorization>,
    watch::Receiver<DeviceDoorAuthorization>,
) {
    let (sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    refresh_once(ledger, &sender);
    (sender, receiver)
}

async fn handshake(
    server_config: Arc<rustls::ServerConfig>,
    client_config: Arc<ClientConfig>,
) -> (
    io::Result<tokio_rustls::server::TlsStream<DuplexStream>>,
    io::Result<tokio_rustls::client::TlsStream<DuplexStream>>,
) {
    let (server_stream, client_stream) = tokio::io::duplex(32 * 1024);
    let (server, client) = tokio::join!(
        TlsAcceptor::from(server_config).accept(server_stream),
        TlsConnector::from(client_config)
            .connect(ServerName::try_from("door.test").unwrap(), client_stream),
    );
    let client = match (server.as_ref(), client) {
        (Err(_), Ok(mut stream)) => {
            let mut byte = [0_u8; 1];
            stream.read(&mut byte).await.map(|_| stream)
        }
        (_, client) => client,
    };
    (server, client)
}

fn assert_peer_alert(
    client: io::Result<tokio_rustls::client::TlsStream<DuplexStream>>,
    alert: AlertDescription,
) {
    let error = client.unwrap_err();
    let rustls_error = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<RustlsError>());
    assert!(matches!(
        rustls_error,
        Some(RustlsError::AlertReceived(received)) if *received == alert
    ));
}

fn fingerprint(certificate: &CertificateDer<'_>) -> String {
    format!("sha256:{}", spl_core::ca::sha256_hex(certificate.as_ref()))
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sol-link-door-integration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
