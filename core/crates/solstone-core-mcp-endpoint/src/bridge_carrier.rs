// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private bridge-carrier control exchange.
//!
//! This is deliberately below the future session/mux owner: it accepts an
//! already-bound in-memory authority and produces no identifier or token
//! accessor. The socket layer can therefore fail closed before SPL bytes are
//! admitted.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ring::signature::Ed25519KeyPair;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_rustls::{TlsConnector, client::TlsStream};

use crate::McpEndpointOwnerContext;
use crate::bridge_pop::{
    BRIDGE_CONTROL_MAX_BYTES, McpBridgePopError, initial_registration_frame, parse_challenge_frame,
    proof_response_frame, renewal_response_frame,
};

/// Bound, memory-only authority passed from the account transition to the
/// bridge control exchange. Its fields stay private to this module.
pub(crate) struct BridgeAuthority {
    token: String,
    hostname: String,
    bridge_id: String,
    bridge_address: String,
    issued_at: i64,
    expires_at: i64,
}

/// Fixed non-secret bindings shared by every authority in one carrier session.
#[derive(Clone)]
pub(crate) struct BridgeBinding {
    hostname: String,
    bridge_id: String,
    bridge_address: String,
}

impl BridgeAuthority {
    pub(crate) fn hostname(&self) -> String {
        self.hostname.clone()
    }

    pub(crate) fn expires_at(&self) -> i64 {
        self.expires_at
    }

    pub(crate) fn renewal_matches(&self, successor: &Self) -> bool {
        successor.hostname == self.hostname
            && successor.bridge_id == self.bridge_id
            && successor.bridge_address == self.bridge_address
            && successor.expires_at > self.expires_at
    }

    pub(crate) fn binding(&self) -> BridgeBinding {
        BridgeBinding {
            hostname: self.hostname.clone(),
            bridge_id: self.bridge_id.clone(),
            bridge_address: self.bridge_address.clone(),
        }
    }

    pub(crate) fn renewal_response(
        &self,
        successor: &Self,
        keypair: &Ed25519KeyPair,
        challenge_frame: &[u8],
        wall_now: i64,
    ) -> Result<Vec<u8>, McpBridgeCarrierError> {
        if !self.renewal_matches(successor) {
            return Err(McpBridgeCarrierError::Pop);
        }
        let challenge = parse_challenge_frame(
            challenge_frame,
            &self.bridge_id,
            self.issued_at,
            self.expires_at,
            wall_now,
        )
        .map_err(map_pop_error)?;
        renewal_response_frame(&successor.token, &successor.hostname, keypair, &challenge)
            .map_err(map_pop_error)
    }
}

impl BridgeBinding {
    pub(crate) fn accepts_successor(
        &self,
        successor: &BridgeAuthority,
        current_expiry: i64,
    ) -> bool {
        successor.hostname == self.hostname
            && successor.bridge_id == self.bridge_id
            && successor.bridge_address == self.bridge_address
            && successor.expires_at > current_expiry
    }
}

impl BridgeAuthority {
    pub(crate) fn new(
        token: String,
        hostname: String,
        bridge_id: String,
        bridge_address: String,
        issued_at: i64,
        expires_at: i64,
    ) -> Self {
        Self {
            token,
            hostname,
            bridge_id,
            bridge_address,
            issued_at,
            expires_at,
        }
    }

    #[cfg(test)]
    fn fixture(
        token: &str,
        hostname: &str,
        bridge_id: &str,
        issued_at: i64,
        expires_at: i64,
    ) -> Self {
        Self {
            token: token.to_owned(),
            hostname: hostname.to_owned(),
            bridge_id: bridge_id.to_owned(),
            bridge_address: "192.0.2.9".to_owned(),
            issued_at,
            expires_at,
        }
    }
}

/// Payload-free failure from the initial bridge control exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpBridgeCarrierError {
    Account,
    Cancelled,
    Deadline,
    Connect,
    Tls,
    Io,
    Pop,
}

impl fmt::Display for McpBridgeCarrierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Account => "MCP bridge registration could not be obtained",
            Self::Cancelled => "MCP bridge connection was cancelled",
            Self::Deadline => "MCP bridge connection exceeded its deadline",
            Self::Connect => "MCP bridge connection failed",
            Self::Tls => "MCP bridge TLS connection failed",
            Self::Io => "MCP bridge control I/O failed",
            Self::Pop => "MCP bridge proof-of-possession failed",
        })
    }
}

impl std::error::Error for McpBridgeCarrierError {}

const BRIDGE_ORIGIN_HOST: &str = "bridge.solstone.me";
const BRIDGE_ORIGIN_PORT: u16 = 443;
const BRIDGE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
const BRIDGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const BRIDGE_TLS_TIMEOUT: Duration = Duration::from_secs(5);

/// An authenticated carrier whose socket, authority, and key material remain opaque.
pub(crate) struct McpBridgeCarrier {
    carrier: TlsStream<TcpStream>,
    authority: BridgeAuthority,
    renewal_owner: McpEndpointOwnerContext,
    shutdown: watch::Receiver<bool>,
}

impl McpBridgeCarrier {
    /// Start the bounded SPL session over this already authenticated carrier.
    pub(crate) fn into_session(self) -> Result<crate::McpBridgeSession, McpBridgeCarrierError> {
        crate::bridge_session::start_bridge_session(
            self.carrier,
            self.authority,
            self.renewal_owner,
            self.shutdown,
        )
    }

    /// Pair the authenticated carrier with TLS state bound to the very same
    /// account-authorized hostname before exposing either to other crates.
    pub(crate) fn into_tunnel(self) -> Result<crate::McpEndpointTunnel, McpBridgeCarrierError> {
        let tls =
            crate::tls::McpEndpointTlsService::for_authorized_hostname(self.authority.hostname());
        let session = self.into_session()?;
        Ok(crate::McpEndpointTunnel { tls, session })
    }
}

/// Connect the fixed bridge IP and finish the first proof exchange.
pub(crate) async fn establish_initial_bridge_carrier(
    authority: BridgeAuthority,
    keypair: &Ed25519KeyPair,
    renewal_owner: McpEndpointOwnerContext,
    shutdown: &mut watch::Receiver<bool>,
    wall_now: i64,
) -> Result<McpBridgeCarrier, McpBridgeCarrierError> {
    if bridge_shutdown_requested(shutdown) {
        return Err(McpBridgeCarrierError::Cancelled);
    }
    let seconds_until_expiry = authority
        .expires_at
        .checked_sub(wall_now)
        .ok_or(McpBridgeCarrierError::Deadline)?;
    if seconds_until_expiry <= 0 {
        return Err(McpBridgeCarrierError::Deadline);
    }
    let started = Instant::now();
    let deadline = (started + BRIDGE_ATTEMPT_TIMEOUT).min(
        started
            + Duration::from_secs(
                u64::try_from(seconds_until_expiry).map_err(|_| McpBridgeCarrierError::Deadline)?,
            ),
    );
    let address = authority
        .bridge_address
        .parse::<Ipv4Addr>()
        .map_err(|_| McpBridgeCarrierError::Connect)?;
    let socket = await_bridge_phase(
        shutdown,
        deadline.min(Instant::now() + BRIDGE_CONNECT_TIMEOUT),
        async move {
            TcpStream::connect(SocketAddr::new(IpAddr::V4(address), BRIDGE_ORIGIN_PORT))
                .await
                .map_err(|_| McpBridgeCarrierError::Connect)
        },
    )
    .await?;
    let tls_deadline = deadline.min(Instant::now() + BRIDGE_TLS_TIMEOUT);
    let server_name = ServerName::try_from(BRIDGE_ORIGIN_HOST.to_owned())
        .map_err(|_| McpBridgeCarrierError::Tls)?;
    let tls_config = bridge_tls_config()?;
    let mut carrier = await_bridge_phase(shutdown, tls_deadline, async move {
        TlsConnector::from(tls_config)
            .connect(server_name, socket)
            .await
            .map_err(|_| McpBridgeCarrierError::Tls)
    })
    .await?;
    await_bridge_phase(
        shutdown,
        deadline,
        prove_initial_bridge_control(&mut carrier, &authority, keypair, wall_now),
    )
    .await?;
    Ok(McpBridgeCarrier {
        carrier,
        authority,
        renewal_owner,
        shutdown: shutdown.clone(),
    })
}

fn bridge_tls_config() -> Result<Arc<ClientConfig>, McpBridgeCarrierError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|_| McpBridgeCarrierError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn bridge_shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

async fn await_bridge_phase<T, F>(
    shutdown: &mut watch::Receiver<bool>,
    deadline: Instant,
    future: F,
) -> Result<T, McpBridgeCarrierError>
where
    F: std::future::Future<Output = Result<T, McpBridgeCarrierError>>,
{
    tokio::pin!(future);
    loop {
        if bridge_shutdown_requested(shutdown) {
            return Err(McpBridgeCarrierError::Cancelled);
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(McpBridgeCarrierError::Deadline)?;
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return Err(McpBridgeCarrierError::Cancelled);
                }
            }
            result = tokio::time::timeout(remaining, &mut future) => {
                return result.map_err(|_| McpBridgeCarrierError::Deadline)?;
            }
        }
    }
}

/// Run the exact registration/challenge/proof exchange on an authenticated TLS carrier.
pub(crate) async fn prove_initial_bridge_control<S>(
    carrier: &mut S,
    authority: &BridgeAuthority,
    keypair: &Ed25519KeyPair,
    wall_now: i64,
) -> Result<(), McpBridgeCarrierError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let registration =
        initial_registration_frame(&authority.token, &authority.hostname).map_err(map_pop_error)?;
    write_bridge_control(carrier, &registration).await?;
    let challenge_frame = read_bridge_control(carrier).await?;
    let challenge = parse_challenge_frame(
        &challenge_frame,
        &authority.bridge_id,
        authority.issued_at,
        authority.expires_at,
        wall_now,
    )
    .map_err(map_pop_error)?;
    let response = proof_response_frame(keypair, &challenge).map_err(map_pop_error)?;
    write_bridge_control(carrier, &response).await
}

async fn read_bridge_control<S>(carrier: &mut S) -> Result<Vec<u8>, McpBridgeCarrierError>
where
    S: AsyncRead + Unpin,
{
    let mut prefix = [0u8; 4];
    carrier
        .read_exact(&mut prefix)
        .await
        .map_err(|_| McpBridgeCarrierError::Io)?;
    let body_length =
        usize::try_from(u32::from_be_bytes(prefix)).map_err(|_| McpBridgeCarrierError::Pop)?;
    if body_length > BRIDGE_CONTROL_MAX_BYTES {
        return Err(McpBridgeCarrierError::Pop);
    }
    let mut frame = Vec::with_capacity(4 + body_length);
    frame.extend_from_slice(&prefix);
    frame.resize(4 + body_length, 0);
    carrier
        .read_exact(&mut frame[4..])
        .await
        .map_err(|_| McpBridgeCarrierError::Io)?;
    Ok(frame)
}

async fn write_bridge_control<S>(writer: &mut S, frame: &[u8]) -> Result<(), McpBridgeCarrierError>
where
    S: AsyncWrite + Unpin,
{
    writer
        .write_all(frame)
        .await
        .map_err(|_| McpBridgeCarrierError::Io)?;
    writer.flush().await.map_err(|_| McpBridgeCarrierError::Io)
}

fn map_pop_error(_: McpBridgePopError) -> McpBridgeCarrierError {
    McpBridgeCarrierError::Pop
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use ring::signature::{Ed25519KeyPair, KeyPair as _};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        BridgeAuthority, McpBridgeCarrierError, await_bridge_phase, prove_initial_bridge_control,
    };

    const HOSTNAME: &str = "aaaqeaye.solstone.me";
    const BRIDGE_ID: &str = "bridge-fixture";
    const ISSUED_AT: i64 = 1_700_000_000;
    const EXPIRES_AT: i64 = 1_700_000_900;

    async fn read_frame<S: AsyncReadExt + Unpin>(carrier: &mut S) -> Vec<u8> {
        let mut prefix = [0u8; 4];
        carrier.read_exact(&mut prefix).await.expect("frame prefix");
        let length = usize::try_from(u32::from_be_bytes(prefix)).expect("frame length");
        let mut frame = prefix.to_vec();
        frame.resize(4 + length, 0);
        carrier
            .read_exact(&mut frame[4..])
            .await
            .expect("frame body");
        frame
    }

    #[tokio::test]
    async fn exact_initial_exchange_frames_and_signs_the_upstream_proof_message() {
        let keypair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).expect("fixture key");
        let public_key = keypair.public_key().as_ref().to_vec();
        let authority =
            BridgeAuthority::fixture("fixture-token", HOSTNAME, BRIDGE_ID, ISSUED_AT, EXPIRES_AT);
        let (mut journal, mut bridge) = tokio::io::duplex(4096);
        let bridge_task = tokio::spawn(async move {
            let registration = read_frame(&mut bridge).await;
            assert_eq!(
                registration,
                [
                    0, 0, 0, 59, b'{', b'\"', b't', b'o', b'k', b'e', b'n', b'\"', b':', b'\"',
                    b'f', b'i', b'x', b't', b'u', b'r', b'e', b'-', b't', b'o', b'k', b'e', b'n',
                    b'\"', b',', b'\"', b'h', b'o', b's', b't', b'n', b'a', b'm', b'e', b'\"',
                    b':', b'\"', b'a', b'a', b'a', b'q', b'e', b'a', b'y', b'e', b'.', b's', b'o',
                    b'l', b's', b't', b'o', b'n', b'e', b'.', b'm', b'e', b'\"', b'}',
                ]
            );
            let challenge = br#"{"nonce":"AAECAwQFBgcICQoLDA0ODw","bridge_id":"bridge-fixture","timestamp":1700000000}"#;
            bridge
                .write_all(
                    &u32::try_from(challenge.len())
                        .expect("challenge length")
                        .to_be_bytes(),
                )
                .await
                .expect("challenge prefix");
            bridge.write_all(challenge).await.expect("challenge body");
            bridge.flush().await.expect("challenge flush");
            let response = read_frame(&mut bridge).await;
            let body: serde_json::Value =
                serde_json::from_slice(&response[4..]).expect("response JSON");
            let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(
                    body.get("signature")
                        .and_then(serde_json::Value::as_str)
                        .expect("signature field"),
                )
                .expect("canonical signature");
            let mut signed =
                Vec::from(&[0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15][..]);
            signed.extend_from_slice(BRIDGE_ID.as_bytes());
            signed.extend_from_slice(&ISSUED_AT.to_be_bytes());
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
                .verify(&signed, &signature)
                .expect("exact bridge proof verifies");
        });

        prove_initial_bridge_control(&mut journal, &authority, &keypair, ISSUED_AT)
            .await
            .expect("journal proves possession");
        bridge_task.await.expect("bridge task completes");
    }

    #[tokio::test]
    async fn control_phases_honor_pre_start_cancellation_and_the_absolute_deadline() {
        let (shutdown_sender, mut cancelled) = tokio::sync::watch::channel(false);
        shutdown_sender.send(true).expect("receiver remains live");
        assert_eq!(
            await_bridge_phase(
                &mut cancelled,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
                std::future::pending::<Result<(), McpBridgeCarrierError>>(),
            )
            .await,
            Err(McpBridgeCarrierError::Cancelled)
        );

        let (_sender, mut active) = tokio::sync::watch::channel(false);
        assert_eq!(
            await_bridge_phase(
                &mut active,
                std::time::Instant::now(),
                std::future::pending::<Result<(), McpBridgeCarrierError>>(),
            )
            .await,
            Err(McpBridgeCarrierError::Deadline)
        );
    }
}
