// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Explicit synthetic fixtures for process restart and test-CA validation.
use super::*;
use axum::http;
use hyper::body::Body;
use instant_acme::{BodyWrapper, BytesResponse, HttpClient};
use serde_json::{Value, json};
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use tokio::io::AsyncWriteExt;

const STAGING_DIRECTORY: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";
const LOCAL_DIRECTORY: &str = "https://localhost:14000/dir";

struct RecordingClient {
    inner: Arc<dyn HttpClient>,
    events: Arc<Mutex<std::fs::File>>,
    held_marker: Option<PathBuf>,
}

impl HttpClient for RecordingClient {
    fn request(
        &self,
        req: http::Request<BodyWrapper<bytes::Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        let inner = Arc::clone(&self.inner);
        let events = Arc::clone(&self.events);
        let held_marker = self.held_marker.clone();
        Box::pin(async move {
            let (parts, mut body) = req.into_parts();
            let mut bytes = Vec::new();
            while let Some(frame) =
                futures::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await
            {
                let frame = frame.expect("infallible request body");
                if let Ok(data) = frame.into_data() {
                    bytes.extend_from_slice(&data);
                }
            }
            let jws: Option<Value> = serde_json::from_slice(&bytes).ok();
            let decode = |field: &str| -> Option<Value> {
                let encoded = jws.as_ref()?.get(field)?.as_str()?;
                let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(encoded)
                    .ok()?;
                serde_json::from_slice(&decoded).ok()
            };
            let payload = decode("payload");
            let protected = decode("protected");
            let method = parts.method.to_string();
            let uri = parts.uri.to_string();
            let finalize = parts.uri.path().contains("/finalize-order/")
                || parts.uri.path().contains("/finalize/");
            let req = http::Request::from_parts(parts, BodyWrapper::from(bytes));
            let result = inner.request(req).await;
            let event = match &result {
                Ok(response) => json!({
                    "at": solstone_core_time::utc_rfc3339(jiff::Timestamp::now()), "method": method, "uri": uri,
                    "status": response.parts.status.as_u16(),
                    "location": response.parts.headers.get("location").and_then(|v| v.to_str().ok()),
                    "retry_after": response.parts.headers.get("retry-after").and_then(|v| v.to_str().ok()),
                    "kid": protected.as_ref().and_then(|v| v.get("kid")),
                    "identifiers": payload.as_ref().and_then(|v| v.get("identifiers")),
                    "replaces": payload.as_ref().and_then(|v| v.get("replaces")),
                }),
                Err(_) => {
                    json!({"at": solstone_core_time::utc_rfc3339(jiff::Timestamp::now()), "method": method, "uri": uri, "transport_error": true})
                }
            };
            {
                let mut file = events.lock().expect("event lock");
                file.write_all(format!("{event}\n").as_bytes())
                    .expect("write event");
                file.sync_data().expect("sync event");
            }
            if finalize
                && result.as_ref().is_ok_and(|r| r.parts.status.is_success())
                && let Some(marker) = held_marker
            {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(marker)
                {
                    Ok(mut file) => {
                        file.write_all(format!("{event}\n").as_bytes())
                            .expect("hold marker");
                        file.sync_all().expect("sync hold marker");
                        status("FINALIZATION_RESPONSE_HELD");
                        std::future::pending::<()>().await;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => panic!("hold marker failed: {e}"),
                }
            }
            result
        })
    }
}

fn required(name: &str) -> String {
    std::env::var(format!("SOLSTONE_ACME_TEST_{name}")).expect(name)
}

#[tokio::test]
#[ignore = "explicit synthetic test-CA fixture required"]
async fn synthetic_acme_process_driver() {
    use std::os::unix::fs::PermissionsExt;
    let container = PathBuf::from(required("ROOT"));
    let directory = required("DIRECTORY");
    let hostname = required("HOSTNAME");
    let nonce = required("NONCE");
    let lane = required("LANE");
    let action = required("ACTION");
    assert!(container.is_absolute());
    assert!(
        container
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("synthetic-renewal-")
    );
    assert!(nonce.len() >= 16);
    assert!(matches!(lane.as_str(), "operated" | "byo"));
    assert!(matches!(action.as_str(), "prepare" | "run"));
    assert!(directory == STAGING_DIRECTORY || directory == LOCAL_DIRECTORY);
    assert!(if directory == STAGING_DIRECTORY {
        hostname.ends_with(".caa-rehearsal.solstone.info")
    } else {
        hostname.ends_with(".test")
    });
    let marker_path = container.join("fixture.json");
    let journal_path = container.join("journal");
    if action == "prepare" {
        std::fs::create_dir(&container).expect("fresh fixture container");
        std::fs::set_permissions(&container, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(&journal_path).unwrap();
        std::fs::set_permissions(&journal_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    } else {
        let marker: Value = serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
        assert_eq!(marker["nonce"], nonce);
        assert_eq!(marker["directory"], directory);
        assert_eq!(marker["hostname"], hostname);
        assert_eq!(marker["lane"], lane);
    }
    let root = Arc::new(JournalRoot::open(&journal_path).expect("fixture journal"));
    let byo_dir = unix::open_byo_directory(&root).unwrap();
    let account_dir = if lane == "byo" {
        unix::open_byo_account_directory(&byo_dir, &hostname).unwrap()
    } else {
        unix::open_tls_state_directory(&root).unwrap()
    };
    let service = if lane == "byo" {
        let cert_dir = unix::open_byo_cert_directory(&byo_dir, &hostname, 1).unwrap();
        McpEndpointTlsService::for_byo_cert_directory_in_environment(
            cert_dir,
            hostname.clone(),
            1,
            McpEndpointCertificateEnvironment::Staging,
        )
        .unwrap()
    } else {
        McpEndpointTlsService::for_authorized_hostname(
            Arc::clone(&root),
            hostname.clone(),
            McpEndpointCertificateEnvironment::Staging,
            false,
        )
        .unwrap()
    };
    let inner: Arc<dyn HttpClient> = if directory == LOCAL_DIRECTORY {
        let roots = std::fs::read(required("ROOT_PEM")).unwrap();
        let roots = pem::parse_many(&roots)
            .unwrap()
            .into_iter()
            .map(|p| rustls::pki_types::CertificateDer::from(p.into_contents()))
            .collect();
        Arc::new(crate::acme_http::McpEndpointHttpClient::with_extra_roots(
            roots,
        ))
    } else {
        assert!(std::env::var_os("SOLSTONE_ACME_TEST_ROOT_PEM").is_none());
        Arc::new(crate::acme_http::McpEndpointHttpClient::new())
    };
    let events = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(container.join("http.jsonl"))
        .unwrap();
    let client: Arc<dyn HttpClient> = Arc::new(RecordingClient {
        inner,
        events: Arc::new(Mutex::new(events)),
        held_marker: (std::env::var("SOLSTONE_ACME_TEST_HOLD_FINALIZE").as_deref() == Ok("1"))
            .then(|| container.join("finalize-held.json")),
    });
    service.set_custom_directory_url(directory.clone());
    service.set_custom_http_client(Arc::clone(&client));
    if action == "prepare" {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .unwrap()
            .serialize_der();
        if lane == "byo" {
            unix::persist_byo_account_key(&account_dir, &key).unwrap();
        } else {
            unix::persist_tls_acme_account_bytes(&account_dir, false, &key).unwrap();
        }
        let pkcs8 = rustls::pki_types::PrivatePkcs8KeyDer::from(key);
        let signing = instant_acme::Key::from_pkcs8_der(pkcs8.clone_key()).unwrap();
        let (account, _) = instant_acme::Account::builder_with_http(Box::new(
            crate::acme_http::ArcHttpClient(Arc::clone(&client)),
        ))
        .create_from_key(
            (signing, rustls::pki_types::PrivateKeyDer::Pkcs8(pkcs8)),
            directory.clone(),
        )
        .await
        .unwrap();
        if lane == "byo" {
            unix::persist_byo_account_uri(&account_dir, account.id()).unwrap();
        }
        let marker = json!({"nonce":nonce,"directory":directory,"hostname":hostname,"lane":lane,"account_uri":account.id()});
        std::fs::write(&marker_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
        std::fs::set_permissions(&marker_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        status(&format!("FIXTURE_PREPARED {marker}"));
        return;
    }
    let port = required("TLS_PORT").parse::<u16>().unwrap();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(mcp_endpoint_server_config(&service));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    if std::env::var("SOLSTONE_ACME_TEST_STOP_AFTER_INSTALL").as_deref() == Ok("1") {
        *service.test_install_shutdown.lock().unwrap() = Some(shutdown_tx.clone());
    }
    let mut server_shutdown = shutdown_rx.clone();
    let server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = server_shutdown.changed() => break,
                result = listener.accept() => {
                    let (stream, _) = result.unwrap();
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        if let Ok(mut tls) = acceptor.accept(stream).await {
                            let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            let _ = tls.shutdown().await;
                        }
                    });
                }
            }
        }
    });
    status(&format!("FIXTURE_LISTENING {port}"));
    let mut renewal_shutdown = shutdown_rx;
    let renewal = async {
        if lane == "byo" {
            service
                .run_byo_acme_renewal(&account_dir, &mut renewal_shutdown)
                .await
        } else if directory == STAGING_DIRECTORY {
            service
                .run_acme_renewal_with_owner_state(&journal_path, &mut renewal_shutdown)
                .await
        } else {
            let marker: Value =
                serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
            let key = unix::read_tls_acme_account_bytes(&account_dir, false)
                .unwrap()
                .unwrap();
            crate::acme_lifecycle::run_renewal_lifecycle(crate::acme_lifecycle::RenewalLoopParams {
                journal_root: Some(&journal_path),
                directory: &service.store.as_ref().unwrap().directory,
                environment: McpEndpointCertificateEnvironment::Staging,
                hostname: &hostname,
                generation: None,
                account_kid: marker["account_uri"].as_str().unwrap().to_owned(),
                account_key_pkcs8: key,
                tls_service: &service,
                shutdown: &mut renewal_shutdown,
                http_client: client,
                picker: None,
            })
            .await
        }
    };
    tokio::pin!(renewal);
    let mut terminate =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    let seconds = std::env::var("SOLSTONE_ACME_TEST_SECONDS")
        .unwrap_or_else(|_| "600".into())
        .parse::<u64>()
        .unwrap();
    assert!((1..=900).contains(&seconds));
    tokio::select! {
        result = &mut renewal => result.expect("renewal lifecycle"),
        _ = terminate.recv() => {
            shutdown_tx.send(true).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut renewal).await.unwrap().unwrap();
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(seconds)) => {
            shutdown_tx.send(true).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut renewal).await.unwrap().unwrap();
        }
    }
    let _ = shutdown_tx.send(true);
    server.await.unwrap();
    status("FIXTURE_STOPPED");
}

fn status(message: &str) {
    let path = std::env::var("SOLSTONE_ACME_TEST_STATUS_LOG").expect("operator fixture status log");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("status log");
    file.write_all(format!("{message}\n").as_bytes())
        .expect("fixture status");
    file.sync_data().expect("sync fixture status");
}
