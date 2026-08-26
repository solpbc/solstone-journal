// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{
    CertificateParams, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, HandshakeKind, RootCertStore};
use solstone_core_convey_http::identity::LinkedDeviceCid;
use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
use solstone_core_sol_link::test_support::{
    FIXED_CERTIFICATE_PEM, FIXED_CERTIFICATE_SHA256, TestCa,
};
use solstone_core_sol_link::{build_device_door_acceptor, serve_device_door_connection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use x509_parser::pem::parse_x509_pem;

const REQUEST: &str = "GET /missing HTTP/1.1\r\nHost: door.test\r\nConnection: close\r\n\r\n";
#[tokio::test]
async fn concurrent_authorized_devices_observe_their_own_distinct_cids() {
    let temporary = TempDir::new();
    let fixture = Fixture::new();
    let first = issue_identity(&fixture.ca, ExtendedKeyUsagePurpose::ClientAuth);
    let second = issue_identity(&fixture.ca, ExtendedKeyUsagePurpose::ClientAuth);
    let (acceptor, refresh) = fixture.acceptor(temporary.path(), &[&first, &second]);

    let (first_response, second_response) = tokio::join!(
        request(
            acceptor.clone(),
            temporary.path(),
            fixture.client_config(&first),
            REQUEST
        ),
        request(
            acceptor,
            temporary.path(),
            fixture.client_config(&second),
            REQUEST
        ),
    );

    let first_cid = cid_for(&first.certificate);
    let second_cid = cid_for(&second.certificate);
    assert_ne!(first_cid, second_cid);
    assert_response_identity(&first_response.0, &first_cid);
    assert_response_identity(&second_response.0, &second_cid);
    refresh.abort();
}

#[tokio::test]
async fn fixed_certificate_digest_is_the_cert_der_sha256_literal() {
    let (_, pem) = parse_x509_pem(FIXED_CERTIFICATE_PEM.as_bytes()).unwrap();
    let certificate = CertificateDer::from(pem.contents);
    let cid = cid_for(&certificate);
    let expected = format!("sha256:{FIXED_CERTIFICATE_SHA256}");

    assert_eq!(cid, expected);
    assert_eq!(
        LinkedDeviceCid::try_from(cid.as_str()).unwrap().as_str(),
        expected
    );
}

#[tokio::test]
async fn request_data_cannot_replace_the_tls_derived_cid() {
    let temporary = TempDir::new();
    let fixture = Fixture::new();
    let client = issue_identity(&fixture.ca, ExtendedKeyUsagePurpose::ClientAuth);
    let (acceptor, refresh) = fixture.acceptor(temporary.path(), &[&client]);
    let expected = cid_for(&client.certificate);
    let alternate = format!("sha256:{}", "b".repeat(64));
    let requests = [
        format!(
            "GET /missing HTTP/1.1\r\nHost: door.test\r\nX-Device-Did: {alternate}\r\nConnection: close\r\n\r\n"
        ),
        format!("GET /{alternate} HTTP/1.1\r\nHost: door.test\r\nConnection: close\r\n\r\n"),
        format!(
            "GET /missing?did={alternate} HTTP/1.1\r\nHost: door.test\r\nConnection: close\r\n\r\n"
        ),
        format!(
            "POST /missing HTTP/1.1\r\nHost: door.test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{alternate}",
            alternate.len()
        ),
    ];

    for request_text in requests {
        let response = request(
            acceptor.clone(),
            temporary.path(),
            fixture.client_config(&client),
            &request_text,
        )
        .await;
        assert_response_identity(&response.0, &expected);
        assert!(!response.0.contains(&alternate));
    }
    refresh.abort();
}

#[tokio::test]
async fn router_receives_one_complete_access_basis_identity() {
    let temporary = TempDir::new();
    let fixture = Fixture::new();
    let client = issue_identity(&fixture.ca, ExtendedKeyUsagePurpose::ClientAuth);
    let (acceptor, refresh) = fixture.acceptor(temporary.path(), &[&client]);
    let expected = cid_for(&client.certificate);

    let response = request(
        acceptor,
        temporary.path(),
        fixture.client_config(&client),
        REQUEST,
    )
    .await;

    assert_response_identity(&response.0, &expected);
    assert_eq!(
        response.0.matches("LinkedDevice { carrier: Direct").count(),
        1
    );
    assert_eq!(response.0.matches(&expected).count(), 1);
    refresh.abort();
}

#[tokio::test]
async fn resumed_tls_session_carries_the_original_connection_cid() {
    let temporary = TempDir::new();
    let fixture = Fixture::new();
    let client = issue_identity(&fixture.ca, ExtendedKeyUsagePurpose::ClientAuth);
    let (acceptor, refresh) = fixture.acceptor(temporary.path(), &[&client]);
    let client_config = fixture.client_config(&client);

    let first = request(
        acceptor.clone(),
        temporary.path(),
        client_config.clone(),
        REQUEST,
    )
    .await;
    let second = request(acceptor, temporary.path(), client_config, REQUEST).await;

    assert_ne!(first.1, HandshakeKind::Resumed);
    assert_eq!(second.1, HandshakeKind::Resumed);
    assert_eq!(response_cid(&first.0), response_cid(&second.0));
    refresh.abort();
}

struct Fixture {
    ca: TestCa,
    server: Identity,
}

impl Fixture {
    fn new() -> Self {
        let ca = TestCa::new();
        let server = issue_identity(&ca, ExtendedKeyUsagePurpose::ServerAuth);
        Self { ca, server }
    }

    fn acceptor(
        &self,
        journal_root: &Path,
        clients: &[&Identity],
    ) -> (TlsAcceptor, JoinHandle<()>) {
        let mut ledger = AuthorizationLedger::new(journal_root);
        for (index, client) in clients.iter().enumerate() {
            ledger
                .add(ClientEntry::new(
                    cid_for(&client.certificate),
                    format!("device-{index}"),
                    "2026-01-01T00:00:00Z",
                    "instance",
                    ClientRole::Roleless,
                ))
                .unwrap();
        }
        build_device_door_acceptor(
            ledger,
            vec![self.server.certificate.clone()],
            self.server.key.clone_key(),
            CertificateDer::from(self.ca.certificate().der().to_vec()),
        )
        .unwrap()
    }

    fn client_config(&self, identity: &Identity) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(self.ca.certificate().der().to_vec()))
            .unwrap();
        Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_client_auth_cert(vec![identity.certificate.clone()], identity.key.clone_key())
                .unwrap(),
        )
    }
}

struct Identity {
    certificate: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

fn issue_identity(ca: &TestCa, usage: ExtendedKeyUsagePurpose) -> Identity {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::new(vec!["door.test".to_string()]).unwrap();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![usage];
    let certificate = params.signed_by(&key, ca.certificate(), ca.key()).unwrap();
    Identity {
        certificate: CertificateDer::from(certificate.der().to_vec()),
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    }
}

async fn request(
    acceptor: TlsAcceptor,
    journal_root: &Path,
    client_config: Arc<ClientConfig>,
    request: &str,
) -> (String, HandshakeKind) {
    let (server, client) = tokio::io::duplex(32 * 1024);
    let journal_root = journal_root.to_path_buf();
    let task = tokio::spawn(async move {
        serve_device_door_connection(server, acceptor, &journal_root)
            .await
            .unwrap();
    });
    let mut stream = TlsConnector::from(client_config)
        .connect(ServerName::try_from("door.test").unwrap(), client)
        .await
        .unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let handshake_kind = stream.get_ref().1.handshake_kind().unwrap();
    task.await.unwrap();

    (String::from_utf8(bytes).unwrap(), handshake_kind)
}

fn cid_for(certificate: &CertificateDer<'_>) -> String {
    format!("sha256:{}", spl_core::ca::sha256_hex(certificate.as_ref()))
}

fn response_cid(response: &str) -> String {
    let start = response.find("sha256:").unwrap();
    response[start..start + "sha256:".len() + 64].to_owned()
}

fn assert_response_identity(response: &str, expected: &str) {
    assert!(response.contains("LinkedDevice { carrier: Direct, cid: LinkedDeviceCid"));
    assert_eq!(response_cid(response), expected);
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sol-link-door-identity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
