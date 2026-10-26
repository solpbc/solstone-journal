// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(unix, test, feature = "full-tests"))]

use std::fs::{self, Permissions};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::Extension;
use chrono::Utc;
use ring::rand::SecureRandom;
use russh::server::Server;
use serde_json::{Value, json};
use solstone_core_journal_io::journal_root::JournalRoot;
use tempfile::TempDir;
use tokio::sync::{Mutex, mpsc, watch};

use crate::byo_dns::{BYO_TEST_SEAM, DnsVerdict, DnsVerdictCode, TEST_VERDICT_OVERRIDE};
use crate::byo_door::{
    BYO_DOOR_STATE_PATH, ByoDoorState, read_byo_door_state, run_byo_door_async,
    write_byo_door_state,
};
use crate::byo_relay::{
    ensure_or_read_client_key, execute_byo_relay_check, generate_remove_script,
    generate_setup_script, read_known_host,
};
use crate::owner_web::{byo_outside_check, byo_relay_check, byo_relay_trust_again, set_byo_relay};
use crate::unix;

static BYO_FULL_TEST_SERIAL: Mutex<()> = Mutex::const_new(());

struct OverridesResetGuard;
impl Drop for OverridesResetGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = TEST_VERDICT_OVERRIDE.write() {
            *g = None;
        }
        if let Ok(mut g) = BYO_TEST_SEAM.outside_check_addr_override.write() {
            *g = None;
        }
        if let Ok(mut g) = BYO_TEST_SEAM.relay_dial_addr_override.write() {
            *g = None;
        }
        if let Ok(mut g) = BYO_TEST_SEAM.acme_spawn_observer.write() {
            *g = None;
        }
        BYO_TEST_SEAM.tick_hold.store(false, Ordering::SeqCst);
        BYO_TEST_SEAM.check_hold.store(false, Ordering::SeqCst);
        BYO_TEST_SEAM
            .dns_lookup_stall
            .store(false, Ordering::SeqCst);
        BYO_TEST_SEAM.client_handle_exit.store(0, Ordering::SeqCst);
        BYO_TEST_SEAM
            .ingress_dial_hold
            .store(false, Ordering::SeqCst);
        BYO_TEST_SEAM
            .ingress_dial_entered
            .store(0, Ordering::SeqCst);
        BYO_TEST_SEAM
            .ingress_dial_connected
            .store(0, Ordering::SeqCst);
        crate::tls::set_test_now_override(None);
    }
}

fn create_test_journal() -> (TempDir, JournalRoot) {
    let dir = tempfile::Builder::new()
        .prefix("solstone-byo-relay-full-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let root = JournalRoot::open(dir.path()).unwrap();
    (dir, root)
}

fn generate_ed25519_keypair() -> russh::keys::PrivateKey {
    let mut seed = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut seed)
        .expect("rng ok");
    let keypair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let keypair_data = russh::keys::ssh_key::private::KeypairData::from(keypair);
    russh::keys::PrivateKey::new(keypair_data, "").expect("private key ok")
}

// ============================================================================
// 1. Setup and remove script execution
// ============================================================================

#[test]
fn test_setup_and_remove_scripts() {
    let _serial = BYO_FULL_TEST_SERIAL.blocking_lock();

    let root_dir = tempfile::Builder::new()
        .prefix("relay-script-root-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let root = root_dir.path();

    let pub_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestKey solstone journal";
    let setup = generate_setup_script(root, pub_key);
    let remove = generate_remove_script(root);

    // Both scripts pass sh -n
    let sh_setup = Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(&setup)
        .output()
        .expect("sh -n setup executes");
    assert!(sh_setup.status.success(), "setup script fails sh -n");

    let sh_remove = Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(&remove)
        .output()
        .expect("sh -n remove executes");
    assert!(sh_remove.status.success(), "remove script fails sh -n");

    // Public key appears once, on restrict line
    let key_matches: Vec<_> = setup.lines().filter(|l| l.contains(pub_key)).collect();
    assert_eq!(key_matches.len(), 1, "public key must appear exactly once");
    assert!(
        key_matches[0].contains(r#"restrict,port-forwarding,permitlisten="0.0.0.0:443""#),
        "key line must carry restrict and permitlisten options"
    );

    // The drop-in ends with Match all
    assert!(setup.contains("Match all\nEOF"));

    // Set up shim environment
    let shim_dir = tempfile::Builder::new()
        .prefix("relay-shims-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    let state_dir = tempfile::Builder::new()
        .prefix("relay-state-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let s_path = state_dir.path();

    let user_file = s_path.join("user_exists");
    let reloaded_file = s_path.join("reloaded");
    let sysctl_val_file = s_path.join("sysctl_val");
    let fail_sshd_file = s_path.join("fail_sshd");
    let sshd_t_file = s_path.join("sshd_t_output");

    // Default sysctl port start = 1024
    fs::write(&sysctl_val_file, "1024").unwrap();

    let write_shim = |name: &str, body: &str| {
        let p = shim_dir.path().join(name);
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, Permissions::from_mode(0o755)).unwrap();
    };

    write_shim(
        "id",
        r#"#!/bin/sh
if [ "${1:-}" = "-u" ]; then
    echo 0
    exit 0
fi
exit 0
"#,
    );

    write_shim(
        "getent",
        &format!(
            r#"#!/bin/sh
if [ -f "{}" ]; then
    echo "solstone-relay:x:1001:1001::/var/lib/solstone-relay:/usr/sbin/nologin"
    exit 0
else
    exit 1
fi
"#,
            user_file.display()
        ),
    );

    write_shim(
        "useradd",
        &format!(
            r#"#!/bin/sh
: > "{}"
exit 0
"#,
            user_file.display()
        ),
    );

    write_shim(
        "userdel",
        &format!(
            r#"#!/bin/sh
/bin/rm -f "{}"
exit 0
"#,
            user_file.display()
        ),
    );

    write_shim(
        "mkdir",
        r#"#!/bin/sh
exec /bin/mkdir "$@"
"#,
    );

    write_shim(
        "chmod",
        r#"#!/bin/sh
exec /bin/chmod "$@"
"#,
    );

    write_shim(
        "chown",
        r#"#!/bin/sh
exit 0
"#,
    );

    write_shim(
        "sysctl",
        &format!(
            r#"#!/bin/sh
if [ "${{1:-}}" = "-n" ]; then
    if [ -f "{}" ]; then
        read -r val < "{}"
        echo "$val"
    else
        echo 1024
    fi
    exit 0
fi
if [ "${{1:-}}" = "-w" ]; then
    val="${{2#*=}}"
    echo "$val" > "{}"
    exit 0
fi
exit 0
"#,
            sysctl_val_file.display(),
            sysctl_val_file.display(),
            sysctl_val_file.display()
        ),
    );

    write_shim(
        "sshd",
        &format!(
            r#"#!/bin/sh
if [ -f "{}" ]; then
    exit 1
fi
if [ "${{1:-}}" = "-t" ]; then
    exit 0
fi
if [ "${{1:-}}" = "-T" ]; then
    while IFS= read -r line || [ -n "$line" ]; do
        echo "$line"
    done < "{}"
    exit 0
fi
exit 0
"#,
            fail_sshd_file.display(),
            sshd_t_file.display()
        ),
    );

    write_shim(
        "systemctl",
        &format!(
            r#"#!/bin/sh
if [ "${{1:-}}" = "is-active" ]; then
    exit 0
fi
if [ "${{1:-}}" = "reload" ]; then
    : > "{}"
    exit 0
fi
exit 0
"#,
            reloaded_file.display()
        ),
    );

    write_shim(
        "rm",
        r#"#!/bin/sh
exec /bin/rm "$@"
"#,
    );

    assert_eq!(
        fs::read_dir(shim_dir.path()).unwrap().count(),
        11,
        "PATH shim must have exactly 11 binaries"
    );

    // Initial sshd_config with Include directive
    let etc_ssh = root.join("etc/ssh");
    fs::create_dir_all(&etc_ssh).unwrap();
    fs::write(
        etc_ssh.join("sshd_config"),
        "Include /etc/ssh/sshd_config.d/*.conf\n",
    )
    .unwrap();

    let fixture_raw = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/sshd_T_capture.txt"),
    )
    .expect("sshd_T_capture.txt exists");

    let clean_fixture = fixture_raw
        .replace("allowtcpforwarding yes", "allowtcpforwarding remote")
        .replace("gatewayports no", "gatewayports clientspecified")
        .replace("clientaliveinterval 0", "clientaliveinterval 15");

    fs::write(&sshd_t_file, &clean_fixture).unwrap();

    let run_setup = || {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(&setup)
            .env_clear()
            .env("PATH", shim_dir.path())
            .output()
            .expect("setup command runs")
    };

    let run_remove = || {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(&remove)
            .env_clear()
            .env("PATH", shim_dir.path())
            .output()
            .expect("remove command runs")
    };

    // Clean run
    let out = run_setup();
    assert!(out.status.success(), "clean setup must succeed");
    let ready_text = String::from_utf8_lossy(&out.stdout);
    assert!(ready_text.contains("ready"), "output must contain ready");
    assert!(reloaded_file.exists(), "reload must occur on success");

    let dropin = root.join("etc/ssh/sshd_config.d/50-solstone-relay.conf");
    assert!(dropin.exists(), "drop-in must exist");
    let dropin_text = fs::read_to_string(&dropin).unwrap();
    assert!(dropin_text.contains("ClientAliveInterval 15"));
    assert!(dropin_text.ends_with("Match all\n"));

    let auth_keys = root.join("var/lib/solstone-relay/.ssh/authorized_keys");
    assert!(auth_keys.exists(), "authorized_keys must exist");
    let auth_text = fs::read_to_string(&auth_keys).unwrap();
    assert_eq!(auth_text.lines().count(), 1, "must have one key line");

    let port_start = root.join("etc/solstone-relay.port-start");
    assert!(port_start.exists(), "port-start record must exist");
    assert_eq!(fs::read_to_string(&port_start).unwrap().trim(), "1024");
    assert_eq!(
        fs::read_to_string(&sysctl_val_file).unwrap().trim(),
        "443",
        "port start lowered to 443"
    );

    // Second run: idempotent
    let out2 = run_setup();
    assert!(out2.status.success(), "second run must succeed");
    assert_eq!(
        fs::read_to_string(&auth_keys).unwrap().lines().count(),
        1,
        "second run leaves one key line"
    );
    assert_eq!(
        fs::read_to_string(&port_start).unwrap().trim(),
        "1024",
        "second run preserves recorded port-start"
    );

    // Negative cases: each one-line edit exits non-zero, never prints ready, leaves no drop-in
    let negative_cases = [
        clean_fixture.replace("disableforwarding no", "disableforwarding yes"),
        format!("{clean_fixture}\nallowusers otheruser"),
        clean_fixture.replace(
            "authenticationmethods any",
            "authenticationmethods publickey,keyboard-interactive",
        ),
        clean_fixture.replace(
            "authorizedkeysfile .ssh/authorized_keys .ssh/authorized_keys2",
            "authorizedkeysfile .ssh/authorized_keys2",
        ),
        clean_fixture.replace("allowtcpforwarding remote", "allowtcpforwarding yes"),
        format!("{clean_fixture}\nallowgroups relaygroup"),
        clean_fixture.replace("clientaliveinterval 15", "clientaliveinterval 0"),
        clean_fixture.replace("gatewayports clientspecified", "gatewayports yes"),
    ];

    for (i, neg) in negative_cases.iter().enumerate() {
        fs::write(&sshd_t_file, neg).unwrap();
        // Remove previous dropin to verify it's not recreated
        let _ = fs::remove_file(&dropin);
        let _ = fs::remove_file(&auth_keys);

        let out_neg = run_setup();
        assert!(
            !out_neg.status.success(),
            "negative case {i} must exit non-zero"
        );
        let neg_text = String::from_utf8_lossy(&out_neg.stdout);
        assert!(
            !neg_text.contains("ready"),
            "negative case {i} must never print ready"
        );
        assert!(
            !dropin.exists(),
            "negative case {i} must leave no drop-in file"
        );
        assert!(
            !auth_keys.exists(),
            "negative case {i} must leave no authorized_keys"
        );
    }

    // Injected failure after trap is installed
    fs::write(&sshd_t_file, &clean_fixture).unwrap();
    fs::write(&sysctl_val_file, "1024").unwrap();
    let _ = fs::remove_file(&port_start);
    fs::write(&fail_sshd_file, "1").unwrap();

    let out_fail = run_setup();
    assert!(!out_fail.status.success(), "injected failure must fail");
    assert!(
        !dropin.exists(),
        "injected failure must clean up drop-in file"
    );
    assert!(
        !auth_keys.exists(),
        "injected failure must leave no authorized_keys"
    );
    assert_eq!(
        fs::read_to_string(&sysctl_val_file).unwrap().trim(),
        "1024",
        "injected failure restores port start"
    );
    let _ = fs::remove_file(&fail_sshd_file);

    // Port start already <= 443 is left alone
    fs::write(&sysctl_val_file, "443").unwrap();
    let _ = fs::remove_file(&port_start);
    let out_low = run_setup();
    assert!(out_low.status.success(), "port start <= 443 must succeed");
    assert!(
        !port_start.exists(),
        "port-start <= 443 must not create port-start file"
    );

    // Remove script execution restores recorded port-start and prints removed
    fs::write(&port_start, "1024\n").unwrap();
    let out_rm = run_remove();
    assert!(out_rm.status.success(), "remove must succeed");
    let rm_text = String::from_utf8_lossy(&out_rm.stdout);
    assert!(rm_text.contains("removed"), "remove must print removed");
    assert!(!port_start.exists(), "remove removes port-start file");
    assert_eq!(
        fs::read_to_string(&sysctl_val_file).unwrap().trim(),
        "1024",
        "remove restores recorded port-start"
    );
}

// ============================================================================
// 2. Outside check
// ============================================================================

#[tokio::test]
async fn test_outside_check() {
    let _serial = BYO_FULL_TEST_SERIAL.lock().await;
    let _guard = OverridesResetGuard;

    let (dir, root_jr) = create_test_journal();
    let root = dir.path().to_path_buf();
    let journal = Arc::new(root.clone());
    let hostname = "mcp.example.com";

    // Set up config
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_string(&json!({
            "mcp_endpoint": {
                "byo_hostname": {
                    "hostname": hostname,
                    "enabled": true,
                    "generation": 1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let byo_dir = unix::open_byo_directory(&root_jr).unwrap();
    let cert_dir = unix::open_byo_cert_directory(&byo_dir, hostname, 1).unwrap();

    // Generate held leaf certificate
    let base_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let not_before = base_now - 1000;
    let not_after = base_now + 10000;

    let held_keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = rcgen::CertificateParams::new(vec![hostname.to_string()]).unwrap();
    params.not_before = time::OffsetDateTime::from_unix_timestamp(not_before).unwrap();
    params.not_after = time::OffsetDateTime::from_unix_timestamp(not_after).unwrap();
    let held_cert = params.self_signed(&held_keypair).unwrap();
    let held_cert_der = held_cert.der().to_vec();
    let held_key_der = held_keypair.serialize_der();

    let tls_service = crate::tls::McpEndpointTlsService::for_byo_cert_directory(
        cert_dir,
        hostname.to_string(),
        1,
    )
    .unwrap();
    crate::tls::set_test_now_override(Some(base_now));
    tls_service
        .install_ordinary_certificate(
            vec![held_cert_der.clone()],
            held_key_der.clone(),
            not_before,
            not_after,
        )
        .unwrap();

    // 1. Held leaf returns outside_ok
    let held_server_config = {
        let certs = vec![rustls::pki_types::CertificateDer::from(
            held_cert_der.clone(),
        )];
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(held_key_der.clone()),
        );
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        Arc::new(
            rustls::ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap(),
        )
    };

    let held_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let held_addr = held_listener.local_addr().unwrap();

    let held_server_handle = tokio::spawn({
        let cfg = held_server_config.clone();
        async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(cfg);
            while let Ok((stream, _)) = held_listener.accept().await {
                let acc = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acc.accept(stream).await {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        use tokio::io::AsyncWriteExt;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        }
    });

    *BYO_TEST_SEAM.outside_check_addr_override.write().unwrap() = Some(held_addr);
    let resp = byo_outside_check(Extension(journal.clone())).await;
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["code"], "outside_ok");

    // 2. Different leaf returns outside_wrong_certificate
    let diff_keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut diff_params = rcgen::CertificateParams::new(vec![hostname.to_string()]).unwrap();
    diff_params.not_before = time::OffsetDateTime::from_unix_timestamp(not_before).unwrap();
    diff_params.not_after = time::OffsetDateTime::from_unix_timestamp(not_after).unwrap();
    let diff_cert = diff_params.self_signed(&diff_keypair).unwrap();

    let diff_server_config = {
        let certs = vec![rustls::pki_types::CertificateDer::from(
            diff_cert.der().to_vec(),
        )];
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(diff_keypair.serialize_der()),
        );
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        Arc::new(
            rustls::ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap(),
        )
    };

    let diff_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let diff_addr = diff_listener.local_addr().unwrap();

    let diff_server_handle = tokio::spawn({
        let cfg = diff_server_config.clone();
        async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(cfg);
            while let Ok((stream, _)) = diff_listener.accept().await {
                let acc = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acc.accept(stream).await {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        use tokio::io::AsyncWriteExt;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        }
    });

    *BYO_TEST_SEAM.outside_check_addr_override.write().unwrap() = Some(diff_addr);
    let resp = byo_outside_check(Extension(journal.clone())).await;
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["code"], "outside_wrong_certificate");

    // 3. Nothing at override address (while held leaf listens) returns outside_unreachable
    let empty_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let empty_addr = empty_listener.local_addr().unwrap();
    drop(empty_listener);

    *BYO_TEST_SEAM.outside_check_addr_override.write().unwrap() = Some(empty_addr);
    let resp = byo_outside_check(Extension(journal.clone())).await;
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["code"], "outside_unreachable");

    // 4. TCP accept that never speaks TLS returns outside_unreachable within 10s
    let stall_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stall_addr = stall_listener.local_addr().unwrap();

    let stall_server_handle = tokio::spawn(async move {
        while let Ok((_stream, _)) = stall_listener.accept().await {
            // Keep stream open, send nothing
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });

    *BYO_TEST_SEAM.outside_check_addr_override.write().unwrap() = Some(stall_addr);
    let start = std::time::Instant::now();
    let resp = byo_outside_check(Extension(journal.clone())).await;
    let elapsed = start.elapsed();
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["code"], "outside_unreachable");
    assert!(
        elapsed <= Duration::from_secs(11),
        "stall check must time out within 10s cap"
    );

    held_server_handle.abort();
    diff_server_handle.abort();
    stall_server_handle.abort();
}

// ============================================================================
// 3. Host-key pin
// ============================================================================

struct SimpleMockSshHandler {
    allow_forward: bool,
    session_handle_tx: Option<tokio::sync::mpsc::UnboundedSender<russh::server::Handle>>,
}

impl russh::server::Handler for SimpleMockSshHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        _key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<russh::server::Auth, Self::Error> {
        if user == "solstone-relay" {
            Ok(russh::server::Auth::Accept)
        } else {
            Ok(russh::server::Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        if let Some(ref tx) = self.session_handle_tx {
            let _ = tx.send(session.handle());
        }
        if self.allow_forward && address == "0.0.0.0" && *port == 443 {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        _port: u32,
        _session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

struct SimpleMockSshServer {
    allow_forward: bool,
    accept_count: Arc<std::sync::atomic::AtomicUsize>,
    session_handle_tx: Option<tokio::sync::mpsc::UnboundedSender<russh::server::Handle>>,
}

impl russh::server::Server for SimpleMockSshServer {
    type Handler = SimpleMockSshHandler;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self::Handler {
        self.accept_count.fetch_add(1, Ordering::SeqCst);
        SimpleMockSshHandler {
            allow_forward: self.allow_forward,
            session_handle_tx: self.session_handle_tx.clone(),
        }
    }
}

async fn start_mock_ssh_server_counted(
    priv_key: russh::keys::PrivateKey,
    port: u16,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    russh::server::RunningServerHandle,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let mut config = russh::server::Config::default();
    config.keys.push(priv_key);
    config.auth_rejection_time = Duration::from_millis(5);
    config.auth_rejection_time_initial = Some(Duration::from_millis(5));

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let accept_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_clone = accept_count.clone();

    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut server = SimpleMockSshServer {
            allow_forward: true,
            accept_count: count_clone,
            session_handle_tx: None,
        };
        let running = server.run_on_socket(Arc::new(config), &listener);
        let _ = handle_tx.send(running.handle());
        let _ = running.await;
    });
    let handle = handle_rx.await.unwrap();

    (addr, task, handle, accept_count)
}

async fn start_mock_ssh_server_counted_with_handles(
    priv_key: russh::keys::PrivateKey,
    port: u16,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    russh::server::RunningServerHandle,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<russh::server::Handle>,
) {
    let mut config = russh::server::Config::default();
    config.keys.push(priv_key);
    config.auth_rejection_time = Duration::from_millis(5);
    config.auth_rejection_time_initial = Some(Duration::from_millis(5));

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let accept_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_clone = accept_count.clone();
    let (session_tx, session_rx) = tokio::sync::mpsc::unbounded_channel();

    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut server = SimpleMockSshServer {
            allow_forward: true,
            accept_count: count_clone,
            session_handle_tx: Some(session_tx),
        };
        let running = server.run_on_socket(Arc::new(config), &listener);
        let _ = handle_tx.send(running.handle());
        let _ = running.await;
    });
    let handle = handle_rx.await.unwrap();

    (addr, task, handle, accept_count, session_rx)
}

async fn start_mock_ssh_server(
    priv_key: russh::keys::PrivateKey,
    port: u16,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    russh::server::RunningServerHandle,
) {
    let (addr, task, handle, _) = start_mock_ssh_server_counted(priv_key, port).await;
    (addr, task, handle)
}

#[tokio::test]
async fn test_host_key_pin() {
    let _serial = BYO_FULL_TEST_SERIAL.lock().await;
    let _guard = OverridesResetGuard;

    let (dir, _root_jr) = create_test_journal();
    let root = dir.path().to_path_buf();
    let journal = Arc::new(root.clone());

    // Create client key in advance
    let _ = ensure_or_read_client_key(&root).unwrap();

    let key1 = generate_ed25519_keypair();
    let key2 = generate_ed25519_keypair();

    let relay_addr: Ipv4Addr = "192.0.2.1".parse().unwrap();

    let (addr1, task1, handle1) = start_mock_ssh_server(key1.clone(), 0).await;
    let port1 = addr1.port();
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(addr1);

    // 1. First connect pins
    let res = execute_byo_relay_check(&root, relay_addr, port1).await;
    assert_eq!(res, Ok(()), "first connect should succeed and pin");
    let pin = read_known_host(&root, relay_addr, port1).unwrap();
    assert!(pin.is_some(), "known-host pin must be present");
    let pin = pin.unwrap();
    assert_eq!(pin.algorithm, "ssh-ed25519");

    handle1.shutdown("restart".to_string());
    let _ = task1.await;

    // 2. Different key at same address yields relay_host_key_changed
    let (addr2, task2, handle2) = start_mock_ssh_server(key2.clone(), port1).await;
    assert_eq!(addr2.port(), port1);
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(addr2);

    let res = execute_byo_relay_check(&root, relay_addr, port1).await;
    assert_eq!(
        res,
        Err("host_key_changed"),
        "different key must yield host_key_changed"
    );

    // No forward, no further attempts until owner acts
    let res_retry = execute_byo_relay_check(&root, relay_addr, port1).await;
    assert_eq!(res_retry, Err("host_key_changed"));

    handle2.shutdown("shutdown".to_string());
    let _ = task2.await;

    // 3. Pin for address X with relay now on a new port: pins anew and does not report host_key_changed
    let (addr3, task3, handle3) = start_mock_ssh_server(key2.clone(), 0).await;
    let port3 = addr3.port();
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(addr3);

    let res3 = execute_byo_relay_check(&root, relay_addr, port3).await;
    assert_eq!(
        res3,
        Ok(()),
        "new port pins anew without host_key_changed error"
    );

    handle3.shutdown("shutdown".to_string());
    let _ = task3.await;

    // 4. trust-again clears the pin and the next connect pins
    let (addr4, task4, handle4) = start_mock_ssh_server(key2.clone(), port1).await;
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(addr4);
    let resp = byo_relay_trust_again(Extension(journal.clone())).await;
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    let val: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(val["ok"], true);

    let pin_cleared = read_known_host(&root, relay_addr, port1).unwrap();
    assert!(
        pin_cleared.is_none(),
        "pin must be cleared after trust-again"
    );

    let res4 = execute_byo_relay_check(&root, relay_addr, port1).await;
    assert_eq!(res4, Ok(()), "next connect pins and succeeds");

    handle4.shutdown("shutdown".to_string());
    let _ = task4.await;

    // 5. An address or port change clears the pin
    let pin_present = read_known_host(&root, relay_addr, port1).unwrap();
    assert!(pin_present.is_some());

    // Change relay via PUT /app/agents/api/byo/relay
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_string(&json!({
            "mcp_endpoint": {
                "byo_hostname": {
                    "hostname": "mcp.example.com",
                    "enabled": true,
                    "generation": 1
                },
                "byo_relay": {
                    "address": "192.0.2.1",
                    "port": port1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let resp_put = set_byo_relay(
        Extension(journal.clone()),
        axum::Json(json!({
            "address": "192.0.2.2",
            "port": port1,
        })),
    )
    .await;
    assert_eq!(resp_put.status(), axum::http::StatusCode::OK);

    let pin_after_change = read_known_host(&root, relay_addr, port1).unwrap();
    assert!(
        pin_after_change.is_none(),
        "address change must clear the pin"
    );
}

// ============================================================================
// 4. Lifetime
// ============================================================================

#[derive(Clone)]
struct StallingAuthServer {
    auth_entered: Arc<tokio::sync::Notify>,
    release: watch::Receiver<bool>,
}

impl russh::server::Handler for StallingAuthServer {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<russh::server::Auth, Self::Error> {
        self.auth_entered.notify_one();
        let _ = self.release.changed().await;
        Ok(russh::server::Auth::Reject {
            proceed_with_methods: None,
            partial_success: false,
        })
    }
}

impl russh::server::Server for StallingAuthServer {
    type Handler = Self;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self::Handler {
        self.clone()
    }
}

#[tokio::test]
async fn stalled_relay_auth_does_not_block_admission_withdrawal_or_shutdown() {
    let _serial = BYO_FULL_TEST_SERIAL.lock().await;
    let _guard = OverridesResetGuard;
    let (dir, root_jr) = create_test_journal();
    let root = dir.path().to_path_buf();
    let hostname = "mcp.example.com";
    let byo_dir = unix::open_byo_directory(&root_jr).unwrap();
    let acc_dir = unix::open_byo_account_directory(&byo_dir, hostname).unwrap();
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    unix::persist_byo_account_key(&acc_dir, &key.serialize_der()).unwrap();
    unix::persist_byo_account_uri(
        &acc_dir,
        "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
    )
    .unwrap();
    let _ = ensure_or_read_client_key(&root).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let auth_entered = Arc::new(tokio::sync::Notify::new());
    let (release_tx, release_rx) = watch::channel(false);
    let mut server = StallingAuthServer {
        auth_entered: Arc::clone(&auth_entered),
        release: release_rx,
    };
    let config = Arc::new(russh::server::Config {
        keys: vec![generate_ed25519_keypair()],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    let (running_tx, running_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let running = server.run_on_socket(config, &listener);
        let _ = running_tx.send(running.handle());
        let _ = running.await;
    });
    let server_handle = running_rx.await.unwrap();
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(address);
    *TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
        DnsVerdict::new(DnsVerdictCode::Admitted, Utc::now()),
        vec!["127.0.0.1".to_string()],
    ));
    let (acme_tx, mut acme_rx) = mpsc::unbounded_channel();
    *BYO_TEST_SEAM.acme_spawn_observer.write().unwrap() = Some(acme_tx);
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_vec(&json!({"mcp_endpoint": {
            "byo_hostname": {"hostname": hostname, "enabled": true, "generation": 1},
            "byo_relay": {"address": "192.0.2.1", "port": address.port()}
        }}))
        .unwrap(),
    )
    .unwrap();

    let mut runtime = crate::byo_door::ByoServiceRuntime::new(
        Arc::new(crate::door_state_writer::DoorStateWriter::new(Arc::new(
            solstone_core_journal_io::JournalRoot::open(&root).unwrap(),
        ))),
        &byo_dir,
        hostname.to_string(),
        1,
    )
    .unwrap();
    let ingress = crate::byo_door::ingress_address(&root);
    runtime
        .tick_service_admission(&root, &byo_dir, &ingress)
        .await;
    tokio::time::timeout(Duration::from_secs(3), auth_entered.notified())
        .await
        .expect("real SSH server reached public-key authentication");
    assert!(
        runtime
            .bound_resources
            .as_ref()
            .unwrap()
            .acme_task
            .is_none()
    );
    assert!(
        acme_rx.try_recv().is_err(),
        "no issuance before relay readiness"
    );

    // Account withdrawal must close ingress even while the remote server withholds auth.
    unix::persist_byo_account_key(&acc_dir, b"invalid-account-key").unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        runtime.tick_service_admission(&root, &byo_dir, &ingress),
    )
    .await
    .expect("withdrawal must cancel stalled authentication immediately");
    assert!(runtime.bound_resources.is_none());
    assert!(!read_byo_door_state(&root).unwrap().socket_listening);
    assert!(tokio::net::UnixStream::connect(&ingress).await.is_err());

    // Restart at a second stalled auth and check explicit service stop too.
    unix::persist_byo_account_key(&acc_dir, &key.serialize_der()).unwrap();
    runtime
        .tick_service_admission(&root, &byo_dir, &ingress)
        .await;
    tokio::time::timeout(Duration::from_secs(3), auth_entered.notified())
        .await
        .expect("second authentication stalls");
    tokio::time::timeout(Duration::from_secs(2), runtime.stop(&byo_dir))
        .await
        .expect("service stop cancels stalled authentication");
    assert!(runtime.bound_resources.is_none());
    assert!(tokio::net::UnixStream::connect(&ingress).await.is_err());
    assert!(
        acme_rx.try_recv().is_err(),
        "stalled relay never starts issuance"
    );
    let _ = release_tx.send(true);
    server_handle.shutdown("done".to_string());
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("fixture server stops")
        .unwrap();
}

async fn notify_tick_and_wait<F>(root: &Path, condition: F) -> ByoDoorState
where
    F: Fn(&ByoDoorState) -> bool,
{
    let prev_ts = read_byo_door_state(root).map(|s| s.observed_at);
    tokio::time::sleep(Duration::from_millis(2)).await;
    BYO_TEST_SEAM.tick_notify.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(state) = read_byo_door_state(root)
                && prev_ts.is_none_or(|ts| state.observed_at > ts)
                && condition(&state)
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("timed out waiting for door tick")
}

#[tokio::test]
async fn test_lifetime() {
    let _serial = BYO_FULL_TEST_SERIAL.lock().await;
    let _guard = OverridesResetGuard;

    let (dir, root_jr) = create_test_journal();
    let root = dir.path().to_path_buf();
    let journal = Arc::new(root.clone());
    let hostname = "mcp.example.com";

    // Prepare valid account
    let byo_dir = unix::open_byo_directory(&root_jr).unwrap();
    let acc_dir = unix::open_byo_account_directory(&byo_dir, hostname).unwrap();
    let acc_keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    unix::persist_byo_account_key(&acc_dir, &acc_keypair.serialize_der()).unwrap();
    unix::persist_byo_account_uri(
        &acc_dir,
        "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
    )
    .unwrap();

    // Create client key in advance
    let _ = ensure_or_read_client_key(&root).unwrap();

    // Admit DNS override
    *TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
        DnsVerdict::new(DnsVerdictCode::Admitted, Utc::now()),
        vec!["127.0.0.1".to_string()],
    ));

    // Phase 0: With no relay configured, a fresh door tick attempts no SSH connection
    // (mock accepts stay 0) and ACME starts on the bind tick.
    let (acme_tx0, mut acme_rx) = mpsc::unbounded_channel();
    *BYO_TEST_SEAM.acme_spawn_observer.write().unwrap() = Some(acme_tx0);
    BYO_TEST_SEAM.tick_hold.store(true, Ordering::SeqCst);

    let (zero_mock_addr, zero_mock_task, zero_mock_handle, zero_accepts) =
        start_mock_ssh_server_counted(generate_ed25519_keypair(), 0).await;
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(zero_mock_addr);

    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_string(&json!({
            "mcp_endpoint": {
                "byo_hostname": {
                    "hostname": hostname,
                    "enabled": true,
                    "generation": 1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let (shutdown_tx0, shutdown_rx0) = watch::channel(false);
    let door_handle0 = tokio::spawn(run_byo_door_async(journal.as_ref().clone(), shutdown_rx0));

    assert_eq!(
        zero_accepts.load(Ordering::SeqCst),
        0,
        "mock sees 0 TCP accepts before tick"
    );
    let state0 = notify_tick_and_wait(&root, |s| s.socket_listening).await;
    assert!(state0.socket_listening);
    assert_eq!(state0.relay_state, "none");
    assert_eq!(
        zero_accepts.load(Ordering::SeqCst),
        0,
        "mock accepts stay 0 with no relay"
    );

    let acme_bind = tokio::time::timeout(Duration::from_secs(2), acme_rx.recv())
        .await
        .expect("ACME starts on bind tick when no relay is configured");
    assert!(acme_bind.is_some());

    let _ = shutdown_tx0.send(true);
    let _ = door_handle0.await;
    zero_mock_handle.shutdown("done".to_string());
    let _ = zero_mock_task.await;

    // Phase 1: Deleting byo_relay while ACME has not started fires ACME on next tick.
    // During backoff the published state stays unreachable, not connecting.
    let (acme_tx1, mut acme_rx) = mpsc::unbounded_channel();
    *BYO_TEST_SEAM.acme_spawn_observer.write().unwrap() = Some(acme_tx1);

    let unreachable_port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() =
        Some(SocketAddr::from(([127, 0, 0, 1], unreachable_port)));

    fs::write(
        root.join("config/journal.json"),
        serde_json::to_string(&json!({
            "mcp_endpoint": {
                "byo_hostname": {
                    "hostname": hostname,
                    "enabled": true,
                    "generation": 1
                },
                "byo_relay": {
                    "address": "192.0.2.1",
                    "port": unreachable_port
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let (shutdown_tx1, shutdown_rx1) = watch::channel(false);
    let door_handle1 = tokio::spawn(run_byo_door_async(journal.as_ref().clone(), shutdown_rx1));

    // Tick to bind ingress and start tunnel to unreachable port
    let state1 = notify_tick_and_wait(&root, |s| s.socket_listening).await;
    assert!(state1.socket_listening);

    // ACME has not started
    assert!(
        acme_rx.try_recv().is_err(),
        "ACME must not start while relay not connected"
    );

    // Refused dial publishes unreachable
    tokio::time::sleep(Duration::from_millis(50)).await;
    let unreach_state = notify_tick_and_wait(&root, |s| s.relay_state == "unreachable").await;
    assert_eq!(
        unreach_state.relay_detail.as_deref(),
        Some("tcp_unreachable")
    );

    // Another tick before the 1s backoff ends still reads unreachable
    let state1_backoff = notify_tick_and_wait(&root, |_| true).await;
    assert_eq!(
        state1_backoff.relay_state, "unreachable",
        "during backoff state stays unreachable"
    );

    // Delete relay while ACME has not started
    let resp_del1 = crate::owner_web::remove_byo_relay(Extension(journal.clone())).await;
    assert_eq!(resp_del1.status(), axum::http::StatusCode::OK);

    // Next tick fires ACME
    let state_after_del = notify_tick_and_wait(&root, |s| s.relay_state == "none").await;
    assert_eq!(state_after_del.relay_state, "none");
    let acme_after_del = tokio::time::timeout(Duration::from_secs(2), acme_rx.recv())
        .await
        .expect("deleting relay while ACME has not started fires ACME on next tick");
    assert!(acme_after_del.is_some());

    let _ = shutdown_tx1.send(true);
    let _ = door_handle1.await;

    // Phase 2: Main lifetime
    let (acme_tx, mut acme_rx) = mpsc::unbounded_channel();
    *BYO_TEST_SEAM.acme_spawn_observer.write().unwrap() = Some(acme_tx);

    let server_key1 = generate_ed25519_keypair();
    let (server_addr1, server_task1, server_handle1, accepts1, mut session_rx1) =
        start_mock_ssh_server_counted_with_handles(server_key1, 0).await;
    let relay_port1 = server_addr1.port();

    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(server_addr1);

    // Configure hostname and relay in 192.0.2.0/24
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_string(&json!({
            "mcp_endpoint": {
                "byo_hostname": {
                    "hostname": hostname,
                    "enabled": true,
                    "generation": 1
                },
                "byo_relay": {
                    "address": "192.0.2.1",
                    "port": relay_port1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let door_handle = tokio::spawn(run_byo_door_async(journal.as_ref().clone(), shutdown_rx));

    // With a relay, the mock sees zero TCP accepts before the tick that binds,
    // and the tunnel connects only after that bind.
    assert_eq!(
        accepts1.load(Ordering::SeqCst),
        0,
        "mock sees zero TCP accepts before tick"
    );

    // Tick once to bind ingress and start relay tunnel
    let bind_state = notify_tick_and_wait(&root, |s| s.socket_listening).await;
    assert!(bind_state.socket_listening);

    // Verify ACME has not started before connected
    assert!(
        acme_rx.try_recv().is_err(),
        "ACME must not start before connected"
    );

    // Wait until tunnel connects to mock server
    let mut session_handle1 = session_rx1.recv().await.unwrap();
    BYO_TEST_SEAM.relay_forward_ready.notified().await;
    assert!(
        accepts1.load(Ordering::SeqCst) >= 1,
        "tunnel connects only after bind"
    );

    // With the tick held, the watch moving to connected leaves the door file unchanged
    let door_state_held = read_byo_door_state(&root).unwrap();
    assert_ne!(
        door_state_held.relay_state, "connected",
        "door file remains unchanged before tick while tick is held"
    );

    // One tick after connected: ACME starts within one tick
    let mut connected_state = notify_tick_and_wait(&root, |s| s.relay_state == "connected").await;
    let acme_spawned = tokio::time::timeout(Duration::from_secs(2), acme_rx.recv())
        .await
        .expect("ACME starts within one tick after connected");
    assert!(acme_spawned.is_some());

    assert_eq!(connected_state.relay_state, "connected");
    assert_eq!(connected_state.dns_verdict.as_deref(), Some("admitted"));
    assert_eq!(connected_state.dns_addresses, vec!["127.0.0.1"]);

    // In-flight dial aborted before completion releases half-open ingress stream:
    // 1. Set ingress_dial_hold
    BYO_TEST_SEAM
        .ingress_dial_hold
        .store(true, Ordering::SeqCst);

    // 2. Open one matching forwarded-tcpip (0.0.0.0:443) on that session
    let hold_ch = session_handle1
        .channel_open_forwarded_tcpip("0.0.0.0", 443, "127.0.0.1", 12345)
        .await
        .expect("channel open succeeds");

    // 3. Wait until entered >= 1 and assert connected == 0
    tokio::time::timeout(Duration::from_secs(3), async {
        while BYO_TEST_SEAM.ingress_dial_entered.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("ingress dial entered");
    assert_eq!(
        BYO_TEST_SEAM.ingress_dial_connected.load(Ordering::SeqCst),
        0
    );

    // 4. Set DNS to non-admitted verdict and run one tick that unbinds
    *TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
        DnsVerdict::new(DnsVerdictCode::Cname, Utc::now()),
        Vec::new(),
    ));
    let unbind_state = notify_tick_and_wait(&root, |s| s.relay_state == "none").await;
    assert_eq!(unbind_state.relay_state, "none");
    assert!(!unbind_state.socket_listening);

    // 5. Assert connected is still 0 and connect to ingress fails
    assert_eq!(
        BYO_TEST_SEAM.ingress_dial_connected.load(Ordering::SeqCst),
        0
    );
    let ingress_target = crate::byo_door::ingress_address(&root);
    #[cfg(unix)]
    let ingress_conn = tokio::net::UnixStream::connect(&ingress_target).await;
    #[cfg(not(unix))]
    let ingress_conn = tokio::net::TcpStream::connect(&ingress_target).await;
    assert!(
        ingress_conn.is_err(),
        "connecting to ingress after unbind tick must fail"
    );

    // 6. Fire ingress_dial_release, wait 100ms, assert connected is still 0
    BYO_TEST_SEAM.ingress_dial_release.notify_waiters();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        BYO_TEST_SEAM.ingress_dial_connected.load(Ordering::SeqCst),
        0,
        "dial that continues after release fails"
    );
    drop(hold_ch);

    // 7. Clear the hold and restore admitted DNS override
    BYO_TEST_SEAM
        .ingress_dial_hold
        .store(false, Ordering::SeqCst);
    *TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
        DnsVerdict::new(DnsVerdictCode::Admitted, Utc::now()),
        vec!["127.0.0.1".to_string()],
    ));

    // Re-bind happens on next admitted tick
    let rebind_state = notify_tick_and_wait(&root, |s| s.socket_listening).await;
    assert!(rebind_state.socket_listening);
    session_handle1 = session_rx1.recv().await.unwrap();
    BYO_TEST_SEAM.relay_forward_ready.notified().await;
    connected_state = notify_tick_and_wait(&root, |s| s.relay_state == "connected").await;
    assert_eq!(connected_state.relay_state, "connected");

    // With a current file of connected, then connecting, then unreachable,
    // the check returns relay_ok, relay_connecting, and unreachable,
    // and the mock sees zero new TCP accepts.
    let base_accepts = accepts1.load(Ordering::SeqCst);

    // connected -> relay_ok
    let resp_ok = byo_relay_check(Extension(journal.clone())).await;
    let body_ok = axum::body::to_bytes(resp_ok.into_body(), 1024)
        .await
        .unwrap();
    let val_ok: Value = serde_json::from_slice(&body_ok).unwrap();
    assert_eq!(val_ok["code"], "relay_ok");
    assert_eq!(accepts1.load(Ordering::SeqCst), base_accepts);

    // connecting -> relay_connecting
    let mut conn_state = connected_state.clone();
    conn_state.relay_state = "connecting".to_string();
    conn_state.observed_at = Utc::now();
    write_byo_door_state(&root, &conn_state);

    let resp_conn = byo_relay_check(Extension(journal.clone())).await;
    let body_conn = axum::body::to_bytes(resp_conn.into_body(), 1024)
        .await
        .unwrap();
    let val_conn: Value = serde_json::from_slice(&body_conn).unwrap();
    assert_eq!(val_conn["code"], "relay_connecting");
    assert_eq!(accepts1.load(Ordering::SeqCst), base_accepts);

    // unreachable -> unreachable
    let mut unreach_state = connected_state.clone();
    unreach_state.relay_state = "unreachable".to_string();
    unreach_state.observed_at = Utc::now();
    write_byo_door_state(&root, &unreach_state);

    let resp_unreach = byo_relay_check(Extension(journal.clone())).await;
    let body_unreach = axum::body::to_bytes(resp_unreach.into_body(), 1024)
        .await
        .unwrap();
    let val_unreach: Value = serde_json::from_slice(&body_unreach).unwrap();
    assert_eq!(val_unreach["code"], "relay_unreachable");
    assert_eq!(
        accepts1.load(Ordering::SeqCst),
        base_accepts,
        "mock sees zero new TCP accepts"
    );

    // A current connected file whose observed_at is 10s old projects state.byo.relay.state none and byo_relay_check dials
    let mut stale_state = connected_state.clone();
    stale_state.observed_at = Utc::now() - chrono::Duration::seconds(10);
    write_byo_door_state(&root, &stale_state);

    let val_st = crate::owner_web::state_value(&root).unwrap();
    assert_eq!(val_st["byo"]["relay"]["state"], "none");

    let _ = byo_relay_check(Extension(journal.clone())).await;
    assert_eq!(
        accepts1.load(Ordering::SeqCst),
        base_accepts + 1,
        "stale check dials"
    );

    // Open an idle accepted forwarded-tcpip channel before DNS becomes not admitted
    let idle_ch = session_handle1
        .channel_open_forwarded_tcpip("0.0.0.0", 443, "127.0.0.1", 12345)
        .await
        .expect("channel open succeeds");

    // After a non-admitted DNS verdict the next file has relay_state none and the check dials.
    // An idle accepted forwarded-tcpip does not delay unbind. One tick closes the session and releases the ingress listener.
    *TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
        DnsVerdict::new(DnsVerdictCode::Cname, Utc::now()),
        Vec::new(),
    ));

    let unbind_t0 = tokio::time::Instant::now();
    let unadmitted_state = notify_tick_and_wait(&root, |s| s.relay_state == "none").await;
    let unbind_elapsed = unbind_t0.elapsed();
    assert!(
        unbind_elapsed < Duration::from_millis(2500),
        "unbind finishes within one tick plus 1s session-abort cap, took {unbind_elapsed:?}"
    );
    assert_eq!(unadmitted_state.relay_state, "none");
    assert!(!unadmitted_state.socket_listening);

    // Connecting to the ingress after that tick fails
    let ingress_target = crate::byo_door::ingress_address(&root);
    #[cfg(unix)]
    let ingress_conn = tokio::net::UnixStream::connect(&ingress_target).await;
    #[cfg(not(unix))]
    let ingress_conn = tokio::net::TcpStream::connect(&ingress_target).await;
    assert!(
        ingress_conn.is_err(),
        "connecting to ingress after unbind tick must fail"
    );

    // Old session closed on unbind
    assert!(
        session_handle1.channel_open_session().await.is_err(),
        "old session closed on unbind"
    );
    drop(idle_ch);

    let accepts_pre_unadmitted = accepts1.load(Ordering::SeqCst);
    let _ = byo_relay_check(Extension(journal.clone())).await;
    assert_eq!(
        accepts1.load(Ordering::SeqCst),
        accepts_pre_unadmitted + 1,
        "check dials after unadmitted DNS"
    );

    // Restore admitted DNS
    *TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
        DnsVerdict::new(DnsVerdictCode::Admitted, Utc::now()),
        vec!["127.0.0.1".to_string()],
    ));

    // A relay PUT to a new documentation address closes the old session within one tick
    // and the next session is only to the new override.
    let server_key2 = generate_ed25519_keypair();
    let (server_addr2, server_task2, server_handle2, accepts2) =
        start_mock_ssh_server_counted(server_key2, 0).await;
    let relay_port2 = server_addr2.port();

    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(server_addr2);

    let resp_put = set_byo_relay(
        Extension(journal.clone()),
        axum::Json(json!({
            "address": "192.0.2.2",
            "port": relay_port2,
        })),
    )
    .await;
    assert_eq!(resp_put.status(), axum::http::StatusCode::OK);

    let old_accepts1 = accepts1.load(Ordering::SeqCst);

    // Tick once to apply PUT and start new tunnel session
    let put_state = notify_tick_and_wait(&root, |s| s.socket_listening).await;
    assert!(put_state.socket_listening);

    tokio::time::timeout(Duration::from_secs(3), async {
        while accepts2.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("next session connects only to new mock");

    assert_eq!(
        accepts1.load(Ordering::SeqCst),
        old_accepts1,
        "old mock gets no new accept"
    );
    assert!(
        session_handle1.channel_open_session().await.is_err(),
        "old mock existing session ends"
    );

    // Pin is for documentation address 192.0.2.2
    let pin_new = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(Some(pin)) =
                crate::byo_relay::read_known_host(&root, "192.0.2.2".parse().unwrap(), relay_port2)
            {
                return pin;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pin is for documentation address 192.0.2.2");
    assert_eq!(pin_new.address, "192.0.2.2");
    let pin_old =
        crate::byo_relay::read_known_host(&root, "192.0.2.1".parse().unwrap(), relay_port1)
            .unwrap();
    assert!(
        pin_old.is_none(),
        "pin for old address was deleted on address change"
    );

    // DELETE while bound stops the tunnel
    let resp_del = crate::owner_web::remove_byo_relay(Extension(journal.clone())).await;
    assert_eq!(resp_del.status(), axum::http::StatusCode::OK);

    let del_state = notify_tick_and_wait(&root, |s| s.relay_state == "none").await;
    assert_eq!(del_state.relay_state, "none");

    let accepts2_after_del = accepts2.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        accepts2.load(Ordering::SeqCst),
        accepts2_after_del,
        "tunnel stopped after DELETE"
    );

    // Clean shutdown
    let _ = shutdown_tx.send(true);
    let _ = door_handle.await;
    server_handle1.shutdown("done".to_string());
    let _ = server_task1.await;
    server_handle2.shutdown("done".to_string());
    let _ = server_task2.await;
}

// ============================================================================
// 5. Hostile server
// ============================================================================

struct HostileServerHandler {
    session_handle_tx: tokio::sync::mpsc::UnboundedSender<russh::server::Handle>,
    return_error_marker: Option<String>,
}

impl russh::server::Handler for HostileServerHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<russh::server::Auth, Self::Error> {
        Ok(russh::server::Auth::Accept)
    }

    async fn tcpip_forward(
        &mut self,
        _address: &str,
        _port: &mut u32,
        session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        if let Some(ref m) = self.return_error_marker {
            return Err(russh::Error::IO(std::io::Error::other(m.clone())));
        }
        let _ = self.session_handle_tx.send(session.handle());
        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        _port: u32,
        _session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

struct HostileServer {
    session_handle_tx: tokio::sync::mpsc::UnboundedSender<russh::server::Handle>,
    return_error_marker: Option<String>,
}

impl russh::server::Server for HostileServer {
    type Handler = HostileServerHandler;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self::Handler {
        HostileServerHandler {
            session_handle_tx: self.session_handle_tx.clone(),
            return_error_marker: self.return_error_marker.clone(),
        }
    }
}

fn russh_err_kind(err: &russh::Error) -> &'static str {
    match err {
        russh::Error::SendError => "SendError",
        russh::Error::Disconnect => "Disconnect",
        russh::Error::ChannelOpenFailure(_) => "ChannelOpenFailure",
        russh::Error::Inconsistent => "Inconsistent",
        russh::Error::IO(_) => "IO",
        _ => "other",
    }
}

#[derive(Debug)]
struct NoCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("tls 1.3 required".to_string()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[tokio::test]
async fn test_hostile_server() {
    let _serial = BYO_FULL_TEST_SERIAL.lock().await;
    let _guard = OverridesResetGuard;

    let (dir, root_jr) = create_test_journal();
    let root = dir.path().to_path_buf();
    let journal = Arc::new(root.clone());
    let hostname = "mcp.example.com";

    let byo_dir = unix::open_byo_directory(&root_jr).unwrap();
    let acc_dir = unix::open_byo_account_directory(&byo_dir, hostname).unwrap();
    let cert_dir = unix::open_byo_cert_directory(&byo_dir, hostname, 1).unwrap();

    let base_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let not_before = base_now - 1000;
    let not_after = base_now + 10000;

    let cert_keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = rcgen::CertificateParams::new(vec![hostname.to_string()]).unwrap();
    params.not_before = time::OffsetDateTime::from_unix_timestamp(not_before).unwrap();
    params.not_after = time::OffsetDateTime::from_unix_timestamp(not_after).unwrap();
    let cert = params.self_signed(&cert_keypair).unwrap();
    let cert_der = cert.der().to_vec();
    let key_der = cert_keypair.serialize_der();

    let acc_keypair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    unix::persist_byo_account_key(&acc_dir, &acc_keypair.serialize_der()).unwrap();
    unix::persist_byo_account_uri(
        &acc_dir,
        "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
    )
    .unwrap();

    let tls_service = crate::tls::McpEndpointTlsService::for_byo_cert_directory(
        cert_dir,
        hostname.to_string(),
        1,
    )
    .unwrap();
    crate::tls::set_test_now_override(Some(base_now));
    tls_service
        .install_ordinary_certificate(vec![cert_der.clone()], key_der, not_before, not_after)
        .unwrap();

    let _ = ensure_or_read_client_key(&root).unwrap();

    // Plant sensitive marker in banner, disconnect description, and server error
    let marker = "SECRET_HOSTILE_BANNER_MARKER_99999";
    let server_key = generate_ed25519_keypair();

    let mut config = russh::server::Config::default();
    config.keys.push(server_key);
    config.server_id = russh::SshId::Standard(format!("SSH-2.0-{marker}").into());
    config.auth_rejection_time = Duration::from_millis(5);
    config.event_buffer_size = 1000;
    config.channel_buffer_size = 1000;

    let (session_handle_tx, mut session_handle_rx) = tokio::sync::mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let (server_running_tx, server_running_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let mut server = HostileServer {
            session_handle_tx,
            return_error_marker: None,
        };
        let running = server.run_on_socket(Arc::new(config), &listener);
        let _ = server_running_tx.send(running.handle());
        let _ = running.await;
    });
    let server_running_handle = server_running_rx.await.unwrap();

    // Decoy listeners at originator and 127.0.0.1:7659
    let decoy_originator = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let decoy_originator_addr = decoy_originator.local_addr().unwrap();
    let decoy_7659 = tokio::net::TcpListener::bind("127.0.0.1:7659").await;

    let decoy_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_clone = decoy_hits.clone();
    let decoy_task = tokio::spawn(async move {
        while let Ok((_s, _)) = decoy_originator.accept().await {
            hits_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Configure relay in 192.0.2.0/24
    *BYO_TEST_SEAM.relay_dial_addr_override.write().unwrap() = Some(server_addr);
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_string(&json!({
            "mcp_endpoint": {
                "byo_hostname": {
                    "hostname": hostname,
                    "enabled": true,
                    "generation": 1
                },
                "byo_relay": {
                    "address": "192.0.2.1",
                    "port": server_addr.port()
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    // 1. Check session refuses matching forwarded-tcpip, and ingress accept count stays 0
    BYO_TEST_SEAM.check_hold.store(true, Ordering::SeqCst);
    let root_check = root.clone();
    let port = server_addr.port();
    let check_task = tokio::spawn(async move {
        execute_byo_relay_check(&root_check, Ipv4Addr::new(192, 0, 2, 1), port).await
    });

    let check_session_handle = session_handle_rx.recv().await.unwrap();
    BYO_TEST_SEAM.relay_forward_ready.notified().await;

    let check_ch = tokio::time::timeout(
        Duration::from_secs(5),
        check_session_handle.channel_open_forwarded_tcpip("0.0.0.0", 443, "127.0.0.1", 12345),
    )
    .await
    .expect("check session timeout");
    let check_kind = check_ch.as_ref().err().map(russh_err_kind).unwrap_or("Ok");
    assert_eq!(
        check_kind, "ChannelOpenFailure",
        "step 4 check-session matching open must be ChannelOpenFailure, got {check_kind}"
    );
    BYO_TEST_SEAM.check_release.notify_one();
    BYO_TEST_SEAM.check_hold.store(false, Ordering::SeqCst);

    let check_res = check_task.await.unwrap();
    assert_eq!(check_res, Ok(()));

    // 2. Door and tunnel session
    *TEST_VERDICT_OVERRIDE.write().unwrap() = Some((
        DnsVerdict::new(DnsVerdictCode::Admitted, Utc::now()),
        vec!["127.0.0.1".to_string()],
    ));
    BYO_TEST_SEAM.tick_hold.store(true, Ordering::SeqCst);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let door_handle = tokio::spawn(run_byo_door_async(journal.as_ref().clone(), shutdown_rx));

    // Tick to bind ingress and connect tunnel
    BYO_TEST_SEAM.tick_notify.notify_one();

    let tunnel_handle = session_handle_rx.recv().await.unwrap();
    BYO_TEST_SEAM.relay_forward_ready.notified().await;

    // Step 1: channel_open_forwarded_tcpip("0.0.0.0", 80, ...) kind is ChannelOpenFailure
    let step1_res = tokio::time::timeout(
        Duration::from_secs(5),
        tunnel_handle.channel_open_forwarded_tcpip("0.0.0.0", 80, "127.0.0.1", 12345),
    )
    .await
    .expect("step 1 timeout");
    let step1_kind = step1_res.as_ref().err().map(russh_err_kind).unwrap_or("Ok");
    if step1_kind != "ChannelOpenFailure" {
        let door_state = read_byo_door_state(&root)
            .map(|s| s.relay_state)
            .unwrap_or_else(|| "err".to_string());
        let client_exit = BYO_TEST_SEAM.client_handle_exit.load(Ordering::SeqCst);
        panic!(
            "step 1 failed with {step1_kind}: relay_state={door_state}, client_exit={client_exit}"
        );
    }

    // Step 2: Then one matching channel_open_forwarded_tcpip("0.0.0.0", 443, ...) is Ok.
    // Do not start the other 39 until this one is Ok.
    let client_cert_verifier = Arc::new(NoCertVerifier);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(client_cert_verifier)
        .with_no_client_auth();
    let tls_connector = tokio_rustls::TlsConnector::from(Arc::new(client_cfg));

    let orig_ip = decoy_originator_addr.ip().to_string();
    let orig_port = decoy_originator_addr.port() as u32;

    let step2_res = tokio::time::timeout(
        Duration::from_secs(5),
        tunnel_handle.channel_open_forwarded_tcpip("0.0.0.0", 443, orig_ip.clone(), orig_port),
    )
    .await
    .expect("step 2 timeout");
    let step2_kind = step2_res.as_ref().err().map(russh_err_kind).unwrap_or("Ok");
    if step2_kind == "SendError" {
        let door_state = read_byo_door_state(&root)
            .map(|s| s.relay_state)
            .unwrap_or_else(|| "err".to_string());
        let client_exit = BYO_TEST_SEAM.client_handle_exit.load(Ordering::SeqCst);
        panic!("step 2 failed with SendError: relay_state={door_state}, client_exit={client_exit}");
    }
    assert_eq!(
        step2_kind, "Ok",
        "step 2 matching forwarded-tcpip port 443 must succeed, got {step2_kind}"
    );

    let ch1 = step2_res.unwrap();
    let stream1 = ch1.into_stream();
    let sn1 = rustls::pki_types::ServerName::try_from(hostname)
        .unwrap()
        .to_owned();
    let mut tls_stream1 =
        tokio::time::timeout(Duration::from_secs(5), tls_connector.connect(sn1, stream1))
            .await
            .expect("step 2 TLS handshake timeout")
            .expect("step 2 TLS handshake to leaf must complete");

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _ = tls_stream1
        .write_all(b"GET / HTTP/1.1\r\nHost: mcp.example.com\r\n\r\n")
        .await;
    let mut buf1 = [0u8; 32];
    let _ = tls_stream1.read(&mut buf1).await;
    let _ = tls_stream1.shutdown().await;

    // Step 3: Only after that one succeeds, assert session, direct-tcpip, x11, and agent are ChannelOpenFailure, and forward_tcpip is Err
    let session_res =
        tokio::time::timeout(Duration::from_secs(5), tunnel_handle.channel_open_session())
            .await
            .expect("step 3 session timeout");
    let session_kind = session_res
        .as_ref()
        .err()
        .map(russh_err_kind)
        .unwrap_or("Ok");
    assert_eq!(
        session_kind, "ChannelOpenFailure",
        "step 3 session open must be ChannelOpenFailure, got {session_kind}"
    );

    let direct_res = tokio::time::timeout(
        Duration::from_secs(5),
        tunnel_handle.channel_open_direct_tcpip("127.0.0.1", 7659, "127.0.0.1", 12345),
    )
    .await
    .expect("step 3 direct_tcpip timeout");
    let direct_kind = direct_res
        .as_ref()
        .err()
        .map(russh_err_kind)
        .unwrap_or("Ok");
    assert_eq!(
        direct_kind, "ChannelOpenFailure",
        "step 3 direct_tcpip open must be ChannelOpenFailure, got {direct_kind}"
    );

    let x11_res = tokio::time::timeout(
        Duration::from_secs(5),
        tunnel_handle.channel_open_x11("127.0.0.1", 12345),
    )
    .await
    .expect("step 3 x11 timeout");
    let x11_kind = x11_res.as_ref().err().map(russh_err_kind).unwrap_or("Ok");
    assert_eq!(
        x11_kind, "ChannelOpenFailure",
        "step 3 x11 open must be ChannelOpenFailure, got {x11_kind}"
    );

    let agent_res =
        tokio::time::timeout(Duration::from_secs(5), tunnel_handle.channel_open_agent())
            .await
            .expect("step 3 agent timeout");
    let agent_kind = agent_res.as_ref().err().map(russh_err_kind).unwrap_or("Ok");
    assert_eq!(
        agent_kind, "ChannelOpenFailure",
        "step 3 agent open must be ChannelOpenFailure, got {agent_kind}"
    );

    let global_res = tokio::time::timeout(
        Duration::from_secs(5),
        tunnel_handle.forward_tcpip("0.0.0.0".to_string(), 443),
    )
    .await
    .expect("step 3 global request timeout");
    assert!(
        global_res.is_err(),
        "step 3 Handle::forward_tcpip global request must return Err"
    );

    // Remaining 39 matching channels complete TLS handshake to leaf (total 40)
    let mut channel_tasks = Vec::new();
    for _ in 1..40 {
        let h = tunnel_handle.clone();
        let conn = tls_connector.clone();
        let sn = rustls::pki_types::ServerName::try_from(hostname)
            .unwrap()
            .to_owned();
        let orig_ip_clone = orig_ip.clone();

        channel_tasks.push(tokio::spawn(async move {
            let ch_res = tokio::time::timeout(
                Duration::from_secs(5),
                h.channel_open_forwarded_tcpip("0.0.0.0", 443, orig_ip_clone, orig_port),
            )
            .await;
            let ch = ch_res
                .expect("timeout on channel_open_forwarded_tcpip")
                .expect("matching forwarded-tcpip must succeed");
            let stream = ch.into_stream();
            let mut tls_stream =
                tokio::time::timeout(Duration::from_secs(5), conn.connect(sn, stream))
                    .await
                    .expect("timeout on TLS handshake")
                    .expect("TLS handshake to leaf must complete");
            let _ = tls_stream
                .write_all(b"GET / HTTP/1.1\r\nHost: mcp.example.com\r\n\r\n")
                .await;
            let mut buf = [0u8; 32];
            let _ = tls_stream.read(&mut buf).await;
            let _ = tls_stream.shutdown().await;
        }));
    }

    for t in channel_tasks {
        t.await.unwrap();
    }

    assert_eq!(
        decoy_hits.load(Ordering::SeqCst),
        0,
        "decoy listeners must accept 0 connections"
    );

    // Plant disconnect description with marker
    let event_before = Utc::now();
    let _ = tunnel_handle
        .disconnect(
            russh::Disconnect::ByApplication,
            marker.to_string(),
            "en".to_string(),
        )
        .await;

    // Wait for tunnel task to notice disconnect
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Tick door to update state
    BYO_TEST_SEAM.tick_notify.notify_one();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let state_file = root.join(BYO_DOOR_STATE_PATH);
    assert!(state_file.exists());
    let state_content = fs::read_to_string(&state_file).unwrap();
    let door_state = read_byo_door_state(&root).unwrap();

    assert_eq!(door_state.relay_state, "unreachable");
    assert!(
        door_state.relay_observed_at.is_some()
            && door_state.relay_observed_at.unwrap() >= event_before,
        "relay_observed_at must be later than event"
    );
    assert!(
        !state_content.contains(marker),
        "marker must be absent from door state file"
    );

    // Marker absent from check response body
    let check_resp = byo_relay_check(Extension(journal.clone())).await;
    let check_body = axum::body::to_bytes(check_resp.into_body(), 1024)
        .await
        .unwrap();
    let check_str = String::from_utf8_lossy(&check_body);
    assert!(
        !check_str.contains(marker),
        "marker must be absent from check response body"
    );

    // Marker absent from state_value JSON
    let st_val = crate::owner_web::state_value(&root).unwrap();
    let st_str = serde_json::to_string(&st_val).unwrap();
    assert!(
        !st_str.contains(marker),
        "marker must be absent from state_value JSON"
    );

    let _ = shutdown_tx.send(true);
    let _ = door_handle.await;
    server_running_handle.shutdown("done".to_string());
    let _ = server_task.await;
    decoy_task.abort();
    drop(decoy_7659);
}
