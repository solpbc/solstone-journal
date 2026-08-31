// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Device-door TLS acceptance and HTTP identity injection.

use std::io;
use std::path::Path;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_convey_http::serve::{serve_connection, tcp_builder};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::DeviceDoorAuthorization;
use crate::door::{
    DeviceDoorConfigError, build_device_door_server_config, spawn_authorization_refresh,
};
use crate::http::router;
use crate::ledger::{AuthorizationLedger, AuthorizedClientsRead};

// Scope transcription of spl-rust v0.5.0 .proto-ref/session.md §7: authorized_clients.json is mtime-polled at 0.5s; revocation propagates within one second of the file edit.
pub const DEVICE_DOOR_AUTHORIZATION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

/// Build a direct device-door TLS acceptor and start its authorization refresh task.
pub fn build_device_door_acceptor(
    ledger: AuthorizationLedger,
    server_cert_chain: Vec<CertificateDer<'static>>,
    server_key: PrivateKeyDer<'static>,
    client_ca: CertificateDer<'static>,
) -> Result<(TlsAcceptor, JoinHandle<()>), DeviceDoorConfigError> {
    let (sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let task =
        spawn_authorization_refresh(ledger, sender, DEVICE_DOOR_AUTHORIZATION_REFRESH_INTERVAL);
    let config = match build_device_door_server_config(
        server_cert_chain,
        server_key,
        client_ca,
        authorization,
    ) {
        Ok(config) => config,
        Err(error) => {
            task.abort();
            return Err(error);
        }
    };
    Ok((TlsAcceptor::from(config), task))
}

/// Accept one direct device-door TLS connection and serve it with its own identity.
pub async fn serve_device_door_connection<I>(
    io: I,
    acceptor: TlsAcceptor,
    journal_root: &Path,
) -> io::Result<()>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let stream = acceptor.accept(io).await?;
    let (_, connection) = stream.get_ref();
    let certificates = connection
        .peer_certificates()
        .ok_or_else(|| io::Error::other("device-door connection has no peer certificate"))?;
    let leaf = certificates.first().ok_or_else(|| {
        io::Error::other("device-door connection has an empty peer certificate chain")
    })?;
    let value = format!("sha256:{}", spl_core::ca::sha256_hex(leaf.as_ref()));
    let cid = LinkedDeviceCid::try_from(value.as_str())
        .map_err(|_| io::Error::other("device-door peer certificate identifier is invalid"))?;
    let builder = tcp_builder();

    serve_connection(
        stream,
        router(journal_root),
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid,
        },
        &builder,
    )
    .await
    .map_err(io::Error::other)
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::time::Duration;

    use super::DEVICE_DOOR_AUTHORIZATION_REFRESH_INTERVAL;

    #[test]
    fn authorization_refresh_interval_is_positive_and_within_the_spec_bound() {
        assert!(DEVICE_DOOR_AUTHORIZATION_REFRESH_INTERVAL > Duration::ZERO);
        assert!(DEVICE_DOOR_AUTHORIZATION_REFRESH_INTERVAL <= Duration::from_millis(500));
    }
}
