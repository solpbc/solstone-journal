// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! BYO SSH remote-forwarding relay client and protocol engine.

use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use ring::rand::SecureRandom;
use russh::keys::PublicKeyBase64;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::unix::{
    BYO_RELAY_CLIENT_KEY_FILE, BYO_RELAY_KNOWN_HOST_FILE, ExclusiveCreate,
    create_exclusive_or_read, delete_byo_relay_known_host, open_byo_directory,
    open_byo_relay_directory, persist_byo_relay_known_host, read_byo_relay_client_key,
    read_byo_relay_known_host,
};

/// Maximum concurrent forwarded channels supported by the tunnel.
pub const MAX_FORWARDED_CHANNELS: usize = 64;

/// Permitted host-key algorithms using pure-Rust ring implementation.
pub const ALLOWED_HOST_KEY_ALGORITHMS: &[&str] = &[
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
];

/// Known-host pin record format stored on disk under `mcp-endpoint/byo/relay/known-host`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownHostPin {
    pub address: String,
    pub port: u16,
    pub algorithm: String,
    pub key_base64: String,
}

/// Live relay status published across the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayStatus {
    pub state: &'static str,
    pub detail: Option<String>,
    pub observed_at: Option<Timestamp>,
}

impl Default for RelayStatus {
    fn default() -> Self {
        Self {
            state: "none",
            detail: None,
            observed_at: None,
        }
    }
}

/// Operational mode of the russh client handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByoRelayMode {
    Tunnel,
    Check,
}

/// Server host-key validation verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyVerdict {
    Accepted,
    HostKeyChanged,
    UnsupportedAlgorithm,
}

/// Channel decrement RAII guard.
struct DecrementOnDrop(Arc<AtomicUsize>);

impl Drop for DecrementOnDrop {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Ingress stream abstraction for Unix domain socket and Windows TCP loopback.
enum IngressStream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
}

impl tokio::io::AsyncRead for IngressStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for IngressStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_flush(cx),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Self::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

async fn dial_ingress_stream(addr: &str) -> io::Result<IngressStream> {
    #[cfg(all(test, feature = "test-hooks"))]
    {
        use std::sync::atomic::Ordering;
        crate::byo_dns::BYO_TEST_SEAM
            .ingress_dial_entered
            .fetch_add(1, Ordering::SeqCst);
        if crate::byo_dns::BYO_TEST_SEAM
            .ingress_dial_hold
            .load(Ordering::SeqCst)
        {
            tokio::select! {
                _ = crate::byo_dns::BYO_TEST_SEAM.ingress_dial_release.notified() => {}
            }
        }
    }

    #[cfg(unix)]
    {
        if addr.starts_with('/') || addr.ends_with(".sock") {
            #[cfg(all(test, feature = "test-hooks"))]
            crate::byo_dns::BYO_TEST_SEAM
                .ingress_dial_connected
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let s = tokio::net::UnixStream::connect(addr).await?;
            return Ok(IngressStream::Unix(s));
        }
    }
    #[cfg(all(test, feature = "test-hooks"))]
    crate::byo_dns::BYO_TEST_SEAM
        .ingress_dial_connected
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let s = tokio::net::TcpStream::connect(addr).await?;
    Ok(IngressStream::Tcp(s))
}

/// Russh client handler for BYO SSH remote-forwarding and health checks.
pub struct ByoRelayHandler {
    mode: ByoRelayMode,
    journal_root: PathBuf,
    address: Ipv4Addr,
    port: u16,
    active_channels: Arc<AtomicUsize>,
    channel_abort_handles: Arc<Mutex<Vec<tokio::task::AbortHandle>>>,
    key_verdict: Arc<Mutex<Option<HostKeyVerdict>>>,
}

impl ByoRelayHandler {
    pub fn new(
        mode: ByoRelayMode,
        journal_root: PathBuf,
        address: Ipv4Addr,
        port: u16,
        active_channels: Arc<AtomicUsize>,
        channel_abort_handles: Arc<Mutex<Vec<tokio::task::AbortHandle>>>,
        key_verdict: Arc<Mutex<Option<HostKeyVerdict>>>,
    ) -> Self {
        Self {
            mode,
            journal_root,
            address,
            port,
            active_channels,
            channel_abort_handles,
            key_verdict,
        }
    }

    fn verify_or_pin_server_key(
        &self,
        server_key: &russh::keys::PublicKey,
    ) -> Result<bool, russh::Error> {
        let binding = server_key.algorithm();
        let algo = binding.as_str();
        if !ALLOWED_HOST_KEY_ALGORITHMS.contains(&algo) {
            *self.key_verdict.lock().unwrap() = Some(HostKeyVerdict::UnsupportedAlgorithm);
            return Ok(false);
        }

        let key_base64 = server_key.public_key_base64();
        let current_addr_str = self.address.to_string();

        let root_res =
            solstone_core_journal_io::journal_root::JournalRoot::open(&self.journal_root)
                .map_err(|_| russh::Error::CouldNotReadKey)?;
        let byo_dir = open_byo_directory(&root_res).map_err(|_| russh::Error::CouldNotReadKey)?;
        let relay_dir =
            open_byo_relay_directory(&byo_dir).map_err(|_| russh::Error::CouldNotReadKey)?;

        let existing_bytes =
            read_byo_relay_known_host(&relay_dir).map_err(|_| russh::Error::CouldNotReadKey)?;

        match existing_bytes {
            Some(bytes) => match serde_json::from_slice::<KnownHostPin>(&bytes) {
                Ok(pin) => {
                    if pin.address == current_addr_str && pin.port == self.port {
                        if pin.algorithm == algo && pin.key_base64 == key_base64 {
                            *self.key_verdict.lock().unwrap() = Some(HostKeyVerdict::Accepted);
                            Ok(true)
                        } else {
                            *self.key_verdict.lock().unwrap() =
                                Some(HostKeyVerdict::HostKeyChanged);
                            Ok(false)
                        }
                    } else {
                        // Pin is for a different address or port; reads as no pin.
                        // Replace pin with current address, port, and key.
                        let new_pin = KnownHostPin {
                            address: current_addr_str,
                            port: self.port,
                            algorithm: algo.to_string(),
                            key_base64,
                        };
                        let json = serde_json::to_vec(&new_pin)
                            .map_err(|_| russh::Error::CouldNotReadKey)?;
                        persist_byo_relay_known_host(&relay_dir, &json)
                            .map_err(|_| russh::Error::CouldNotReadKey)?;
                        *self.key_verdict.lock().unwrap() = Some(HostKeyVerdict::Accepted);
                        Ok(true)
                    }
                }
                Err(_) => {
                    // Corrupted pin for current file: do not replace, do not connect.
                    *self.key_verdict.lock().unwrap() = Some(HostKeyVerdict::HostKeyChanged);
                    Ok(false)
                }
            },
            None => {
                // First creation: use exclusive create.
                let new_pin = KnownHostPin {
                    address: current_addr_str.clone(),
                    port: self.port,
                    algorithm: algo.to_string(),
                    key_base64,
                };
                let json =
                    serde_json::to_vec(&new_pin).map_err(|_| russh::Error::CouldNotReadKey)?;
                match create_exclusive_or_read(&relay_dir, BYO_RELAY_KNOWN_HOST_FILE, &json) {
                    Ok(ExclusiveCreate::Created) => {
                        *self.key_verdict.lock().unwrap() = Some(HostKeyVerdict::Accepted);
                        Ok(true)
                    }
                    Ok(ExclusiveCreate::Present(other_bytes)) => {
                        match serde_json::from_slice::<KnownHostPin>(&other_bytes) {
                            Ok(other_pin) => {
                                if other_pin.address != current_addr_str
                                    || other_pin.port != self.port
                                {
                                    persist_byo_relay_known_host(&relay_dir, &json)
                                        .map_err(|_| russh::Error::CouldNotReadKey)?;
                                    *self.key_verdict.lock().unwrap() =
                                        Some(HostKeyVerdict::Accepted);
                                    Ok(true)
                                } else if other_pin.algorithm == algo
                                    && other_pin.key_base64 == new_pin.key_base64
                                {
                                    *self.key_verdict.lock().unwrap() =
                                        Some(HostKeyVerdict::Accepted);
                                    Ok(true)
                                } else {
                                    *self.key_verdict.lock().unwrap() =
                                        Some(HostKeyVerdict::HostKeyChanged);
                                    Ok(false)
                                }
                            }
                            Err(_) => {
                                *self.key_verdict.lock().unwrap() =
                                    Some(HostKeyVerdict::HostKeyChanged);
                                Ok(false)
                            }
                        }
                    }
                    Err(_) => Err(russh::Error::CouldNotReadKey),
                }
            }
        }
    }
}

impl russh::client::Handler for ByoRelayHandler {
    type Error = russh::Error;

    fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        let res = match server_public_key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                self.verify_or_pin_server_key(key)
            }
            russh::keys::PublicKeyOrCertificate::Certificate(_) => Ok(false),
        };
        async move { res }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        if self.mode != ByoRelayMode::Tunnel {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }

        if connected_address != "0.0.0.0" || connected_port != 443 {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }

        let prev = self.active_channels.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_FORWARDED_CHANNELS {
            self.active_channels.fetch_sub(1, Ordering::SeqCst);
            reply
                .reject(russh::ChannelOpenFailure::ResourceShortage)
                .await;
            return Ok(());
        }

        reply.accept().await;

        let mut channel_stream = channel.into_stream();
        let journal_root = self.journal_root.clone();
        let counter = self.active_channels.clone();

        let handle = tokio::spawn(async move {
            let _guard = DecrementOnDrop(counter);
            let ingress_target = crate::byo_door::ingress_address(&journal_root);
            if let Ok(mut ingress_stream) = dial_ingress_stream(&ingress_target).await {
                let _ =
                    tokio::io::copy_bidirectional(&mut channel_stream, &mut ingress_stream).await;
            }
        });

        self.channel_abort_handles
            .lock()
            .unwrap()
            .push(handle.abort_handle());
        Ok(())
    }

    async fn server_channel_open_session(
        &mut self,
        _channel: russh::Channel<russh::client::Msg>,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn server_channel_open_direct_tcpip(
        &mut self,
        _channel: russh::Channel<russh::client::Msg>,
        _host_to_connect: &str,
        _port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn server_channel_open_direct_streamlocal(
        &mut self,
        _channel: russh::Channel<russh::client::Msg>,
        _socket_path: &str,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        _channel: russh::Channel<russh::client::Msg>,
        _socket_path: &str,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        _channel: russh::Channel<russh::client::Msg>,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        _channel: russh::Channel<russh::client::Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn should_accept_unknown_server_channel(
        &mut self,
        _id: russh::ChannelId,
        _channel_type: &str,
    ) -> bool {
        false
    }

    async fn server_channel_open_unknown(
        &mut self,
        _channel: russh::Channel<russh::client::Msg>,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }

    async fn openssh_ext_host_keys_announced(
        &mut self,
        _keys: Vec<russh::keys::PublicKey>,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn disconnected(
        &mut self,
        _reason: russh::client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Ensure client key exists or create it exclusively. Returns (parsed key, openssh public key).
pub fn ensure_or_read_client_key(
    journal_root: &Path,
) -> io::Result<(russh::keys::PrivateKey, String)> {
    let root_res = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let byo_dir = open_byo_directory(&root_res)?;
    let relay_dir = open_byo_relay_directory(&byo_dir)?;

    let existing = read_byo_relay_client_key(&relay_dir)?;
    if let Some(bytes) = existing {
        let key_str = String::from_utf8(bytes).map_err(|_| io::Error::other("invalid key utf8"))?;
        let parsed = russh::keys::decode_secret_key(&key_str, None)
            .map_err(|_| io::Error::other("invalid client key"))?;
        let pub_str = parsed
            .public_key()
            .to_openssh()
            .map_err(|_| io::Error::other("failed to format public key"))?;
        return Ok((parsed, pub_str.trim().to_string()));
    }

    let mut seed = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut seed)
        .map_err(|_| io::Error::other("rng error"))?;
    let keypair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let keypair_data = russh::keys::ssh_key::private::KeypairData::from(keypair);
    let priv_key = russh::keys::PrivateKey::new(keypair_data, "")
        .map_err(|_| io::Error::other("failed to create private key"))?;

    let openssh_pem = priv_key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .map_err(|_| io::Error::other("failed to format openssh key"))?;

    match create_exclusive_or_read(
        &relay_dir,
        BYO_RELAY_CLIENT_KEY_FILE,
        openssh_pem.as_bytes(),
    )? {
        ExclusiveCreate::Created => {
            let pub_str = priv_key
                .public_key()
                .to_openssh()
                .map_err(|_| io::Error::other("failed to format public key"))?;
            Ok((priv_key, pub_str.trim().to_string()))
        }
        ExclusiveCreate::Present(other_bytes) => {
            let key_str =
                String::from_utf8(other_bytes).map_err(|_| io::Error::other("invalid key utf8"))?;
            let parsed = russh::keys::decode_secret_key(&key_str, None)
                .map_err(|_| io::Error::other("invalid client key"))?;
            let pub_str = parsed
                .public_key()
                .to_openssh()
                .map_err(|_| io::Error::other("failed to format public key"))?;
            Ok((parsed, pub_str.trim().to_string()))
        }
    }
}

/// Load existing client key if present without creating a new one.
pub fn load_client_key(journal_root: &Path) -> io::Result<Option<russh::keys::PrivateKey>> {
    let root_res = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let byo_dir = open_byo_directory(&root_res)?;
    let relay_dir = open_byo_relay_directory(&byo_dir)?;

    let existing = read_byo_relay_client_key(&relay_dir)?;
    match existing {
        Some(bytes) => {
            let key_str =
                String::from_utf8(bytes).map_err(|_| io::Error::other("invalid key utf8"))?;
            let parsed = russh::keys::decode_secret_key(&key_str, None)
                .map_err(|_| io::Error::other("invalid client key"))?;
            Ok(Some(parsed))
        }
        None => Ok(None),
    }
}

/// Delete stored known-host pin.
pub fn delete_known_host(journal_root: &Path) -> io::Result<()> {
    let root_res = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let byo_dir = open_byo_directory(&root_res)?;
    let relay_dir = open_byo_relay_directory(&byo_dir)?;
    delete_byo_relay_known_host(&relay_dir)
}

/// Read stored known-host pin for the given address and port, if it matches.
#[allow(dead_code)]
pub fn read_known_host(
    journal_root: &Path,
    address: Ipv4Addr,
    port: u16,
) -> io::Result<Option<KnownHostPin>> {
    let root_res = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let byo_dir = open_byo_directory(&root_res)?;
    let relay_dir = open_byo_relay_directory(&byo_dir)?;
    match read_byo_relay_known_host(&relay_dir)? {
        Some(bytes) => match serde_json::from_slice::<KnownHostPin>(&bytes) {
            Ok(pin) => {
                if pin.address == address.to_string() && pin.port == port {
                    Ok(Some(pin))
                } else {
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        },
        None => Ok(None),
    }
}

/// Check if the known-host file is absent on disk.
pub fn known_host_file_is_absent(journal_root: &Path) -> bool {
    let Ok(root_res) = solstone_core_journal_io::journal_root::JournalRoot::open(journal_root)
    else {
        return false;
    };
    let Ok(byo_dir) = open_byo_directory(&root_res) else {
        return false;
    };
    let Ok(relay_dir) = open_byo_relay_directory(&byo_dir) else {
        return false;
    };
    matches!(read_byo_relay_known_host(&relay_dir), Ok(None))
}

/// Generate host setup shell script. Uses only allowed external commands.
pub fn generate_setup_script(root: &Path, public_key_openssh: &str) -> String {
    let root_str = root.to_string_lossy();
    let prefix = if root_str == "/" {
        ""
    } else {
        root_str.trim_end_matches('/')
    };

    let sshd_config = format!("{prefix}/etc/ssh/sshd_config");
    let conf_dir = format!("{prefix}/etc/ssh/sshd_config.d");
    let dropin_file = format!("{prefix}/etc/ssh/sshd_config.d/50-solstone-relay.conf");
    let home_dir = format!("{prefix}/var/lib/solstone-relay");
    let ssh_dir = format!("{prefix}/var/lib/solstone-relay/.ssh");
    let auth_keys = format!("{prefix}/var/lib/solstone-relay/.ssh/authorized_keys");
    let port_start_file = format!("{prefix}/etc/solstone-relay.port-start");
    let sysctl_dir = format!("{prefix}/etc/sysctl.d");
    let sysctl_file = format!("{prefix}/etc/sysctl.d/50-solstone-relay.conf");
    let pub_key = public_key_openssh.trim();

    format!(
        r#"#!/bin/sh
# solstone: let agents reach your journal through this server
set -eu

if [ "$(id -u)" -ne 0 ]; then
  echo "run this as an admin (with sudo). nothing was changed."
  exit 1
fi

if [ ! -f "{sshd_config}" ]; then
  echo "this server has no OpenSSH server settings at {sshd_config}. nothing was changed."
  exit 1
fi

has_include=0
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    Include*sshd_config.d/*.conf*|include*sshd_config.d/*.conf*)
      has_include=1
      break
      ;;
  esac
done < "{sshd_config}"

if [ "$has_include" -ne 1 ]; then
  echo "this server's SSH settings don't read {conf_dir}, so your journal's login can't be limited. nothing was changed."
  exit 1
fi

if ! getent passwd solstone-relay >/dev/null 2>&1; then
  useradd -s /usr/sbin/nologin -d "{home_dir}" -p '*' solstone-relay
fi

mkdir -p "{home_dir}"
chmod 0755 "{home_dir}"
chown solstone-relay:solstone-relay "{home_dir}"
mkdir -p "{ssh_dir}"
chmod 0700 "{ssh_dir}"
chown solstone-relay:solstone-relay "{ssh_dir}"

changed_port_start=0

cleanup() {{
  rm -f "{dropin_file}" "{auth_keys}"
  if [ "$changed_port_start" -eq 1 ]; then
    if [ -f "{port_start_file}" ]; then
      read -r orig_val < "{port_start_file}" || [ -n "$orig_val" ]
      if [ -n "$orig_val" ]; then
        sysctl -w net.ipv4.ip_unprivileged_port_start="$orig_val" >/dev/null 2>&1 || true
      fi
      rm -f "{sysctl_file}" "{port_start_file}"
    fi
  fi
}}

trap cleanup EXIT HUP INT TERM

mkdir -p "{conf_dir}"
while IFS= read -r line || [ -n "$line" ]; do
  echo "$line"
done << 'EOF' > "{dropin_file}"
Match User solstone-relay
    AllowTcpForwarding remote
    GatewayPorts clientspecified
    X11Forwarding no
    AllowAgentForwarding no
    PermitTTY no
    ClientAliveInterval 15
    ClientAliveCountMax 3
Match all
EOF

curr_port=$(sysctl -n net.ipv4.ip_unprivileged_port_start 2>/dev/null || echo 0)
if [ "$curr_port" -gt 443 ]; then
  if [ ! -f "{port_start_file}" ]; then
    echo "$curr_port" > "{port_start_file}"
  fi
  mkdir -p "{sysctl_dir}"
  while IFS= read -r line || [ -n "$line" ]; do
    echo "$line"
  done << 'EOF' > "{sysctl_file}"
net.ipv4.ip_unprivileged_port_start = 443
EOF
  sysctl -w net.ipv4.ip_unprivileged_port_start=443 >/dev/null 2>&1 || true
  changed_port_start=1
fi

sshd -t

# addr=127.0.0.1 cannot see Match Address blocks for the journal's real address.
val_allowtcpforwarding=""
val_gatewayports=""
val_disableforwarding=""
val_pubkeyauthentication=""
val_clientaliveinterval=""
val_authorizedkeysfile=""
val_authenticationmethods=""
val_denyusers=""
val_denygroups=""
val_permitlisten=""
val_allowgroups=""
val_allowusers=""

while read -r k v || [ -n "$k" ]; do
  case "$k" in
    allowtcpforwarding) val_allowtcpforwarding="$v" ;;
    gatewayports) val_gatewayports="$v" ;;
    disableforwarding) val_disableforwarding="$v" ;;
    pubkeyauthentication) val_pubkeyauthentication="$v" ;;
    clientaliveinterval) val_clientaliveinterval="$v" ;;
    authorizedkeysfile) val_authorizedkeysfile="$v" ;;
    authenticationmethods) val_authenticationmethods="$v" ;;
    denyusers) val_denyusers="$v" ;;
    denygroups) val_denygroups="$v" ;;
    permitlisten) val_permitlisten="$v" ;;
    allowgroups) val_allowgroups="$v" ;;
    allowusers) val_allowusers="$v" ;;
  esac
done << EOF
$(sshd -T -C user=solstone-relay,host=localhost,addr=127.0.0.1)
EOF

if [ "$val_allowtcpforwarding" != "remote" ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: allowtcpforwarding is $val_allowtcpforwarding"
  exit 1
fi

if [ "$val_gatewayports" != "clientspecified" ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: gatewayports is $val_gatewayports"
  exit 1
fi

if [ "$val_disableforwarding" != "no" ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: disableforwarding is $val_disableforwarding"
  exit 1
fi

if [ "$val_pubkeyauthentication" != "yes" ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: pubkeyauthentication is $val_pubkeyauthentication"
  exit 1
fi

if [ -z "$val_clientaliveinterval" ] || [ "$val_clientaliveinterval" = "0" ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: clientaliveinterval is $val_clientaliveinterval"
  exit 1
fi

has_auth_key=0
for tok in $val_authorizedkeysfile; do
  if [ "$tok" = ".ssh/authorized_keys" ]; then
    has_auth_key=1
    break
  fi
done
if [ "$has_auth_key" -ne 1 ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: authorizedkeysfile is $val_authorizedkeysfile"
  exit 1
fi

if [ "$val_authenticationmethods" != "any" ] && [ "$val_authenticationmethods" != "publickey" ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: authenticationmethods is $val_authenticationmethods"
  exit 1
fi

for u in $val_denyusers; do
  if [ "$u" = "solstone-relay" ]; then
    rm -f "{dropin_file}"
    echo "this server still blocks your journal's forward: denyusers is $val_denyusers"
    exit 1
  fi
done

for g in $val_denygroups; do
  if [ "$g" = "solstone-relay" ]; then
    rm -f "{dropin_file}"
    echo "this server still blocks your journal's forward: denygroups is $val_denygroups"
    exit 1
  fi
done

has_permitlisten=0
if [ "$val_permitlisten" = "any" ]; then
  has_permitlisten=1
else
  for tok in $val_permitlisten; do
    case "$tok" in
      0.0.0.0:443|*:443|443|any)
        has_permitlisten=1
        break
        ;;
    esac
  done
fi
if [ "$has_permitlisten" -ne 1 ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: permitlisten is $val_permitlisten"
  exit 1
fi

if [ -n "$val_allowgroups" ]; then
  rm -f "{dropin_file}"
  echo "this server still blocks your journal's forward: allowgroups is $val_allowgroups"
  exit 1
fi

if [ -n "$val_allowusers" ]; then
  has_user=0
  for u in $val_allowusers; do
    if [ "$u" = "solstone-relay" ]; then
      has_user=1
      break
    fi
  done
  if [ "$has_user" -ne 1 ]; then
    rm -f "{dropin_file}"
    echo "this server still blocks your journal's forward: allowusers is $val_allowusers"
    exit 1
  fi
fi

if systemctl is-active --quiet ssh >/dev/null 2>&1; then
  systemctl reload ssh
elif systemctl is-active --quiet sshd >/dev/null 2>&1; then
  systemctl reload sshd
fi

while IFS= read -r line || [ -n "$line" ]; do
  echo "$line"
done << 'EOF' > "{auth_keys}"
restrict,port-forwarding,permitlisten="0.0.0.0:443" {pub_key} solstone journal
EOF
chmod 0600 "{auth_keys}"
chown solstone-relay:solstone-relay "{auth_keys}"

if command -v restorecon >/dev/null 2>&1; then
  restorecon -R "{ssh_dir}" >/dev/null 2>&1 || true
fi

trap - EXIT HUP INT TERM
echo "ready"
"#
    )
}

/// Generate host removal shell script. Uses only allowed external commands.
pub fn generate_remove_script(root: &Path) -> String {
    let root_str = root.to_string_lossy();
    let prefix = if root_str == "/" {
        ""
    } else {
        root_str.trim_end_matches('/')
    };

    let dropin_file = format!("{prefix}/etc/ssh/sshd_config.d/50-solstone-relay.conf");
    let home_dir = format!("{prefix}/var/lib/solstone-relay");
    let port_start_file = format!("{prefix}/etc/solstone-relay.port-start");
    let sysctl_file = format!("{prefix}/etc/sysctl.d/50-solstone-relay.conf");

    format!(
        r#"#!/bin/sh
set -eu

pkill -u solstone-relay >/dev/null 2>&1 || true

rm -f "{dropin_file}"
rm -f "{sysctl_file}"

if [ -f "{port_start_file}" ]; then
  read -r orig_val < "{port_start_file}" || [ -n "$orig_val" ]
  if [ -n "$orig_val" ]; then
    sysctl -w net.ipv4.ip_unprivileged_port_start="$orig_val" >/dev/null 2>&1 || true
  fi
  rm -f "{port_start_file}"
fi

rm -rf "{home_dir}"
userdel solstone-relay >/dev/null 2>&1 || true

sshd -t >/dev/null 2>&1 || true

if systemctl is-active --quiet ssh >/dev/null 2>&1; then
  systemctl reload ssh || true
elif systemctl is-active --quiet sshd >/dev/null 2>&1; then
  systemctl reload sshd || true
fi

echo "removed"
"#
    )
}

/// Execute a one-shot check against the SSH relay.
pub async fn execute_byo_relay_check(
    journal_root: &Path,
    address: Ipv4Addr,
    port: u16,
) -> Result<(), &'static str> {
    let client_key = match load_client_key(journal_root) {
        Ok(Some(k)) => k,
        Ok(None) => return Err("auth_refused"),
        Err(_) => return Err("auth_refused"),
    };

    let key_verdict = Arc::new(Mutex::new(None));
    let active_channels = Arc::new(AtomicUsize::new(0));
    let abort_handles = Arc::new(Mutex::new(Vec::new()));

    let handler = ByoRelayHandler::new(
        ByoRelayMode::Check,
        journal_root.to_path_buf(),
        address,
        port,
        active_channels,
        abort_handles,
        key_verdict.clone(),
    );

    let config = Arc::new(russh::client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    });

    let connect_future = {
        #[cfg(all(test, feature = "test-hooks"))]
        let dial_addr = crate::byo_dns::BYO_TEST_SEAM
            .relay_dial_addr_override
            .read()
            .unwrap()
            .map(|sa| (sa.ip().to_string(), sa.port()))
            .unwrap_or_else(|| (address.to_string(), port));
        #[cfg(not(all(test, feature = "test-hooks")))]
        let dial_addr = (address.to_string(), port);
        russh::client::connect(config, dial_addr, handler)
    };
    let mut handle = match tokio::time::timeout(Duration::from_secs(5), connect_future).await {
        Ok(Ok(h)) => h,
        Ok(Err(_)) => {
            if let Some(HostKeyVerdict::HostKeyChanged) = *key_verdict.lock().unwrap() {
                return Err("host_key_changed");
            }
            return Err("unreachable");
        }
        Err(_) => return Err("unreachable"),
    };

    let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(client_key), None);
    let auth_res = handle
        .authenticate_publickey("solstone-relay", key_with_alg)
        .await;
    match auth_res {
        Ok(russh::client::AuthResult::Success) => {}
        _ => return Err("auth_refused"),
    }

    match handle.tcpip_forward("0.0.0.0", 443).await {
        Ok(_) => {
            #[cfg(all(test, feature = "test-hooks"))]
            crate::byo_dns::BYO_TEST_SEAM
                .relay_forward_ready
                .notify_one();

            #[cfg(all(test, feature = "test-hooks"))]
            if crate::byo_dns::BYO_TEST_SEAM
                .check_hold
                .load(Ordering::SeqCst)
            {
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    crate::byo_dns::BYO_TEST_SEAM.check_release.notified(),
                )
                .await;
            }
            let _ = handle.cancel_tcpip_forward("0.0.0.0", 443).await;
            Ok(())
        }
        Err(russh::Error::RequestDenied) => Err("forward_refused"),
        Err(_) => Err("forward_refused"),
    }
}

async fn abort_and_wait_channel_tasks(
    channel_abort_handles: &Arc<Mutex<Vec<tokio::task::AbortHandle>>>,
    active_channels: &Arc<AtomicUsize>,
) {
    let handles = {
        let mut list = channel_abort_handles.lock().unwrap();
        std::mem::take(&mut *list)
    };
    for h in handles {
        h.abort();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while active_channels.load(Ordering::SeqCst) > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Run the background SSH tunnel loop with backoff and cancellation.
pub async fn run_byo_relay_tunnel(
    journal_root: PathBuf,
    address: Ipv4Addr,
    port: u16,
    mut shutdown_rx: watch::Receiver<bool>,
    status_tx: watch::Sender<RelayStatus>,
    channel_abort_handles: Arc<Mutex<Vec<tokio::task::AbortHandle>>>,
) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);
    let active_channels = Arc::new(AtomicUsize::new(0));

    while !*shutdown_rx.borrow() {
        let client_key = match load_client_key(&journal_root) {
            Ok(Some(k)) => k,
            _ => {
                let _ = status_tx.send(RelayStatus {
                    state: "auth_refused",
                    detail: Some("no_client_key".to_string()),
                    observed_at: Some(Timestamp::now()),
                });
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(backoff) => {
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                }
            }
        };

        let _ = status_tx.send(RelayStatus {
            state: "connecting",
            detail: None,
            observed_at: Some(Timestamp::now()),
        });

        let key_verdict = Arc::new(Mutex::new(None));

        let handler = ByoRelayHandler::new(
            ByoRelayMode::Tunnel,
            journal_root.clone(),
            address,
            port,
            active_channels.clone(),
            channel_abort_handles.clone(),
            key_verdict.clone(),
        );

        let config = Arc::new(russh::client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        });

        let connect_future = {
            #[cfg(all(test, feature = "test-hooks"))]
            let dial_addr = crate::byo_dns::BYO_TEST_SEAM
                .relay_dial_addr_override
                .read()
                .unwrap()
                .map(|sa| (sa.ip().to_string(), sa.port()))
                .unwrap_or_else(|| (address.to_string(), port));
            #[cfg(not(all(test, feature = "test-hooks")))]
            let dial_addr = (address.to_string(), port);
            russh::client::connect(config, dial_addr, handler)
        };
        let connected = tokio::select! {
            _ = shutdown_rx.changed() => break,
            result = tokio::time::timeout(Duration::from_secs(5), connect_future) => result,
        };
        let mut handle = match connected {
            Ok(Ok(h)) => h,
            Ok(Err(_)) => {
                let is_host_key_changed =
                    key_verdict.lock().unwrap().as_ref() == Some(&HostKeyVerdict::HostKeyChanged);
                if is_host_key_changed {
                    let _ = status_tx.send(RelayStatus {
                        state: "host_key_changed",
                        detail: Some("host_key_mismatch".to_string()),
                        observed_at: Some(Timestamp::now()),
                    });
                    // Do not retry host_key_changed inside the tunnel task. Wait for shutdown.
                    tokio::select! {
                        _ = shutdown_rx.changed() => break,
                        _ = std::future::pending::<()>() => {}
                    }
                    break;
                }
                let _ = status_tx.send(RelayStatus {
                    state: "unreachable",
                    detail: Some("tcp_unreachable".to_string()),
                    observed_at: Some(Timestamp::now()),
                });
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(backoff) => {
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                }
            }
            Err(_) => {
                let _ = status_tx.send(RelayStatus {
                    state: "unreachable",
                    detail: Some("tcp_unreachable".to_string()),
                    observed_at: Some(Timestamp::now()),
                });
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(backoff) => {
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                }
            }
        };

        let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(client_key), None);
        let auth_res = tokio::select! {
            _ = shutdown_rx.changed() => break,
            result = tokio::time::timeout(
                Duration::from_secs(5),
                handle.authenticate_publickey("solstone-relay", key_with_alg),
            ) => result,
        };
        match auth_res {
            Ok(Ok(russh::client::AuthResult::Success)) => {}
            _ => {
                let _ = status_tx.send(RelayStatus {
                    state: "auth_refused",
                    detail: Some("auth_failed".to_string()),
                    observed_at: Some(Timestamp::now()),
                });
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(backoff) => {
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                }
            }
        }

        let forwarded = tokio::select! {
            _ = shutdown_rx.changed() => break,
            result = tokio::time::timeout(
                Duration::from_secs(5),
                handle.tcpip_forward("0.0.0.0", 443),
            ) => result,
        };
        match forwarded {
            Ok(Ok(_)) => {
                backoff = Duration::from_secs(1);
                #[cfg(all(test, feature = "test-hooks"))]
                crate::byo_dns::BYO_TEST_SEAM
                    .relay_forward_ready
                    .notify_one();

                let _ = status_tx.send(RelayStatus {
                    state: "connected",
                    detail: None,
                    observed_at: Some(Timestamp::now()),
                });

                let mut pinned = std::pin::pin!(handle);
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        abort_and_wait_channel_tasks(&channel_abort_handles, &active_channels).await;
                        let _ = tokio::time::timeout(Duration::from_secs(1), pinned.cancel_tcpip_forward("0.0.0.0", 443)).await;
                        let _ = tokio::time::timeout(Duration::from_secs(1), pinned.disconnect(russh::Disconnect::ByApplication, "", "")).await;
                        break;
                    }
                    res = &mut pinned => {
                        #[cfg(all(test, feature = "test-hooks"))]
                        crate::byo_dns::BYO_TEST_SEAM.client_handle_exit.store(
                            if res.is_ok() { 1 } else { 2 },
                            Ordering::SeqCst,
                        );
                        abort_and_wait_channel_tasks(&channel_abort_handles, &active_channels).await;
                        let _ = status_tx.send(RelayStatus {
                            state: "unreachable",
                            detail: Some("tcp_unreachable".to_string()),
                            observed_at: Some(Timestamp::now()),
                        });
                        let _ = res;
                        tokio::select! {
                            _ = shutdown_rx.changed() => break,
                            _ = tokio::time::sleep(backoff) => {
                                backoff = (backoff * 2).min(max_backoff);
                                continue;
                            }
                        }
                    }
                }
            }
            _ => {
                let _ = status_tx.send(RelayStatus {
                    state: "forward_refused",
                    detail: Some("forward_rejected".to_string()),
                    observed_at: Some(Timestamp::now()),
                });
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(backoff) => {
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                }
            }
        }
    }

    abort_and_wait_channel_tasks(&channel_abort_handles, &active_channels).await;
}
