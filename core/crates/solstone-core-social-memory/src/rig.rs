// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A rehearsal rig for development builds: an atproto host, identity
//! directory and authorization server running on the developer's own
//! computer under one private domain. Requests to a host under that domain go
//! to the rig's address over TLS that trusts only the rig's certificate
//! authority; a request to any other host is refused, so a rehearsal reaches
//! nothing outside its rig. Every request is appended to the rig's hop log.
//!
//! Compiled only into development builds.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde::Deserialize;
use solstone_core_oauth::atproto_client::HandleDns;
use solstone_core_oauth::ssrf::{
    HttpRequest, HttpResponse, HttpsAttemptIo, OAuthHttpClient, SsrfFetchError, send_with_io,
};

/// Stands in for a rig host's address so the shared HTTPS path accepts it;
/// the connection always goes to the rig's own address instead.
const PLACEHOLDER: IpAddr = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));

/// `rig.json`: where the rig listens and what it trusts.
#[derive(Debug, Clone, Deserialize)]
pub struct RigConfig {
    /// The rig's private domain, such as `sm-rig.test`.
    pub domain: String,
    /// The address the rig's TLS front listens on.
    pub connect: SocketAddr,
    /// The rig certificate authority's PEM file.
    pub ca: PathBuf,
    /// The rig's identity directory, such as `https://plc.sm-rig.test`.
    pub plc: String,
    /// Where each request is logged, one line per hop.
    pub hop_log: PathBuf,
}

impl RigConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display()))
    }

    fn covers(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let domain = self.domain.to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    }
}

/// The rig's HTTPS client.
pub struct RigHttp {
    config: RigConfig,
    tls: Arc<ClientConfig>,
}

impl RigHttp {
    pub fn new(config: RigConfig) -> Result<Self, String> {
        let pem =
            std::fs::read(&config.ca).map_err(|err| format!("{}: {err}", config.ca.display()))?;
        let mut roots = RootCertStore::empty();
        for cert in CertificateDer::pem_slice_iter(&pem) {
            roots
                .add(cert.map_err(|err| format!("rig CA: {err:?}"))?)
                .map_err(|err| format!("rig CA: {err}"))?;
        }
        if roots.is_empty() {
            return Err("rig CA file holds no certificate".to_owned());
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(|err| err.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            config,
            tls: Arc::new(tls),
        })
    }

    pub fn plc(&self) -> &str {
        &self.config.plc
    }

    fn log(&self, request: &HttpRequest, outcome: &str) {
        let target = request.url.split('?').next().unwrap_or("");
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.hop_log)
        {
            let _ = writeln!(file, "{} {target} {outcome}", request.method.as_str());
        }
    }
}

struct RigIo<'a> {
    rig: &'a RigHttp,
}

impl HttpsAttemptIo for RigIo<'_> {
    type Stream = StreamOwned<ClientConnection, TcpStream>;

    fn resolve(&mut self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        if self.rig.config.covers(host) {
            Ok(vec![SocketAddr::new(PLACEHOLDER, port)])
        } else {
            Err(io::Error::other("host is outside the rig"))
        }
    }

    fn connect_tls(
        &mut self,
        address: SocketAddr,
        server_name: &str,
        connect_timeout: Duration,
        io_timeout: Duration,
    ) -> Result<Self::Stream, SsrfFetchError> {
        if address.ip() != PLACEHOLDER || !self.rig.config.covers(server_name) {
            return Err(SsrfFetchError::UnusableAddress);
        }
        let tcp = TcpStream::connect_timeout(&self.rig.config.connect, connect_timeout)
            .map_err(|_| SsrfFetchError::Connect)?;
        let _ = tcp.set_read_timeout(Some(io_timeout));
        let _ = tcp.set_write_timeout(Some(io_timeout));
        let name = ServerName::try_from(server_name.to_owned()).map_err(|_| SsrfFetchError::Tls)?;
        let conn = ClientConnection::new(Arc::clone(&self.rig.tls), name)
            .map_err(|_| SsrfFetchError::Tls)?;
        let mut stream = StreamOwned::new(conn, tcp);
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(|_| SsrfFetchError::Tls)?;
        }
        Ok(stream)
    }
}

impl OAuthHttpClient for RigHttp {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
        let result = send_with_io(request, &mut RigIo { rig: self });
        match &result {
            Ok(response) => self.log(request, &response.status.to_string()),
            Err(err) => self.log(request, &format!("refused: {err}")),
        }
        result
    }
}

/// Rig handles have no DNS records; they resolve by their well-known file.
pub struct RigDns;

impl HandleDns for RigDns {
    fn txt(&self, _name: &str) -> Option<Vec<String>> {
        Some(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RigConfig {
        RigConfig {
            domain: "sm-rig.test".to_owned(),
            connect: "127.0.0.1:8443".parse().unwrap(),
            ca: PathBuf::from("/nonexistent"),
            plc: "https://plc.sm-rig.test".to_owned(),
            hop_log: PathBuf::from("/nonexistent"),
        }
    }

    #[test]
    fn the_rig_covers_its_domain_and_nothing_else() {
        let rig = config();
        assert!(rig.covers("pds.sm-rig.test"));
        assert!(rig.covers("alice.pds.SM-RIG.test"));
        assert!(rig.covers("sm-rig.test"));
        assert!(!rig.covers("plc.directory"));
        assert!(!rig.covers("evilsm-rig.test"));
        assert!(!rig.covers("sm-rig.test.example.com"));
    }
}
