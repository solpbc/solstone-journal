// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rcgen::{CertificateParams, CustomExtension, KeyPair, PKCS_ECDSA_P256_SHA256};
use ring::{
    rand::SystemRandom,
    signature::{RSA_PKCS1_SHA256, RsaKeyPair},
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use serde_json::{Map, json};
use solstone_core_generate::{ContentPart, GenerateRequest, HEALTH_BRAIN_GENERATE_CONTEXT};
use solstone_core_generate_wire::{
    ConfidentialResult, EndpointRuntime, PoolClock,
    test_support::{confidential_generate_attested, confidential_generate_attested_with_readiness},
};
use solstone_core_local::ByoEndpoint;
use solstone_core_spp_attest::{
    CpuBundle,
    nvgpu::claims::GpuAppraisal,
    snp::{CpuAppraisal, CpuTcb, TcbVersion},
};
use solstone_core_spp_ratls::{
    CompositeVerdict, CompositeVerificationError, CompositeVerificationInput, CompositeVerifier,
    NvattestEnsureStatus,
    ratls::contract::{
        CompositeEvidence, EXPORTER_BYTES, EXPORTER_LABEL, EXPORTER_PROOF_MEDIA_TYPE,
        ExporterProof, exporter_binding, exporter_context,
    },
};
use x509_parser::prelude::FromDer;

const TEST_AK_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDd4QO8Gw8LEy9I\nBWtcCN/nqi8GQAwYRMrPKcvR98vqWF6VImZoRQVLotl9ud+k/oSzDH/APeKzdhlH\nyMhHedGJxDProAmegyeIi7TtekICkM811sslns7kyCaCRRV90nQ0Gzmf7WWCsOJX\nRwAH3YQlAJAMfK/SLRnoVb3bG1qwfnCOrNuhsmcMZt7baMH8HpG1/rm39KrRFnow\n5lyIBuoGH9TXuellFb4MTMf8ntXMosqOHe4R3LpfaJ17KjBZkGPPwy+PRiVidfYt\nKkwrCWS30XPfsahxCIF8+aFFmb12fpDvR7g4K+7xrhmfL8SXx00n5C00ucmUSCRT\n0QekpVujAgMBAAECggEADkrHwE6n5ek68vMyaq/BqH0MYWUvwkJwI+8Hy4MgNfyy\nPv4DxbSodipLwy79anXgm13zPrFd0HyLfVXAHOaKakrio0tgQz8khUWmhmOJK/wi\n9M9cr5QutIr1/A8yJrQvOwoD6LrUfpohQkj3BgqtT+rc3IkNlEbGc/JN8/arnVGw\n73DA82n4ajY16mlGBTVVMVmBr77D1as0fAUF8mH5S1mCkmdHz1FJOyEDG3F4C/Gm\nR6ZAnEN/dd4eCTI4GMcfrUZV/LXZuwE2xkUoCMP/7h7S+PA08RIDLZu/b/2c4kZp\nN37Hr+BvmPRpui74NzQszMXvpuLVxTr0/CebRisr5QKBgQD95euOEySX0bsyyQsq\ngwPycxpTJl9h0xUBUuZ69zRXMbYY6tc7xAlQaZSYaNfsFGLvPYxelZGVgXbw+nEI\nKoFuz70AI8HrDX1fb6IOQ6NwGT2qPkqZpySmJC9CQ8WcRwX8sdKVrigHPvI0i4tt\nQh4DOYOgmo1BTnWYRk0EuCEX5wKBgQDftzyvg4WaVp5xyZVyc8bCooMwfbnRKvBk\nYwwpxAJZQssRtWgy6gZ7f1m4wIaxmdNay9oKzMoGEaq6hb23uTe90nNyNtvb69I/\nBdJT6/HA8fmRyyyLqfcBLhpiqVlWWEuk1NHvti96VkjgVz7scVIV34kvEqPV+z/X\nVT/ibvX25QKBgQCaQ3cydIkYQVL3EVXad44PYkYNXVQ4sLKjgkYNUmOX0tlsHEu3\nwW1TUUL6s0D17JEMAR5nXYL+DpJA6jmBF6patJeGHTO2aBTTxpT1C72i34MrC/vx\nja9jzrp0DY9kW3bUyQpE7XLerC0nJd4J/VEU7n3+N8k5c71ZTuV+x4074wKBgCuE\nLR3G65oV90QS/isBMkxx6CrqidaSD6i3S4pkQkCyqWWMb/RXaWNkZkN1z72EOoSS\n2pr3MuTzUs5tbXXrZVhbM3GoEiQ5PvBbZYpFfwUVDIK7jrKsIQvtt9wxLNuK2Uv6\nyctjGOEnH43j6q17bYgrrzek3JGnCcgNIRwekWGxAoGAe6Et5hGK+jLoRmUIvoZw\nBriUxphnVgDIvZpWb3ZYNdKlHHVQh4GwClJ3cfFoTYKjI1KRdUjEAZg8dAmAr+gN\niEljJylwJzlVN6TJ43FQ2UwqcglRHGyir9lyfCaNvDJqAuGSfGF6AhRZl7MSpcou\nE9nC0XWcWIVtpOJj3T+19Jk=\n-----END PRIVATE KEY-----\n";
const TEST_AK_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA3eEDvBsPCxMvSAVrXAjf\n56ovBkAMGETKzynL0ffL6lhelSJmaEUFS6LZfbnfpP6Eswx/wD3is3YZR8jIR3nR\nicQz66AJnoMniIu07XpCApDPNdbLJZ7O5MgmgkUVfdJ0NBs5n+1lgrDiV0cAB92E\nJQCQDHyv0i0Z6FW92xtasH5wjqzbobJnDGbe22jB/B6Rtf65t/Sq0RZ6MOZciAbq\nBh/U17npZRW+DEzH/J7VzKLKjh3uEdy6X2ideyowWZBjz8Mvj0YlYnX2LSpMKwlk\nt9Fz37GocQiBfPmhRZm9dn6Q70e4OCvu8a4Zny/El8dNJ+QtNLnJlEgkU9EHpKVb\nowIDAQAB\n-----END PUBLIC KEY-----\n";

struct AcceptingCompositeVerifier;
struct RejectingCompositeVerifier;

impl CompositeVerifier for AcceptingCompositeVerifier {
    fn verify(
        &self,
        _: CpuBundle<'_>,
        _: CompositeVerificationInput<'_>,
    ) -> Result<CompositeVerdict, CompositeVerificationError> {
        Ok(test_verdict())
    }
}

impl CompositeVerifier for RejectingCompositeVerifier {
    fn verify(
        &self,
        _: CpuBundle<'_>,
        _: CompositeVerificationInput<'_>,
    ) -> Result<CompositeVerdict, CompositeVerificationError> {
        Err(CompositeVerificationError {
            reason_code: "composite_appraisal_failed",
        })
    }
}

struct CountingCompositeVerifier {
    verify_count: Arc<AtomicUsize>,
}

impl CompositeVerifier for CountingCompositeVerifier {
    fn verify(
        &self,
        _: CpuBundle<'_>,
        _: CompositeVerificationInput<'_>,
    ) -> Result<CompositeVerdict, CompositeVerificationError> {
        self.verify_count.fetch_add(1, Ordering::SeqCst);
        Ok(test_verdict())
    }
}

#[derive(Debug)]
struct StaticCertResolver(Arc<CertifiedKey>);

impl ResolvesServerCert for StaticCertResolver {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

#[derive(Debug)]
struct CountingCertResolver {
    key: Arc<CertifiedKey>,
    resolve_count: Arc<AtomicUsize>,
}

impl ResolvesServerCert for CountingCertResolver {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.resolve_count.fetch_add(1, Ordering::SeqCst);
        Some(self.key.clone())
    }
}

#[derive(Debug)]
struct DummyTicketer;

impl rustls::server::ProducesTickets for DummyTicketer {
    fn enabled(&self) -> bool {
        true
    }
    fn lifetime(&self) -> u32 {
        3600
    }
    fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
        Some(plain.to_vec())
    }
    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        Some(cipher.to_vec())
    }
}

fn test_verdict() -> CompositeVerdict {
    let tcb = TcbVersion {
        boot_loader: None,
        tee: None,
        snp: None,
        microcode: None,
        fmc: None,
    };
    CompositeVerdict {
        verified: true,
        legs: ["cpu", "gpu"],
        substrate: String::new(),
        checked_at: SystemTime::UNIX_EPOCH,
        cpu: CpuAppraisal {
            steps: Vec::new(),
            hcla_version: 0,
            report_version: 0,
            cpuid_family: None,
            cpuid_model: None,
            cpuid_step: None,
            tcb: CpuTcb {
                current: tcb.clone(),
                reported: tcb.clone(),
                committed: tcb.clone(),
                launch: tcb,
            },
            pcr_sha256: String::new(),
            host_data_hex: String::new(),
            measurement_hex: String::new(),
            chip_id_hex: String::new(),
        },
        gpu: GpuAppraisal {
            steps: Vec::new(),
            driver_version: String::new(),
            vbios_version: String::new(),
            hwmodel: String::new(),
            ueid: String::new(),
            oemid: String::new(),
            eat_nonce: String::new(),
            claims_version: String::new(),
            arch: String::new(),
            envelope_gpu_uuid: String::new(),
            status: solstone_core_spp_attest::nvgpu::GpuStatusAuthorization::OnlineNonce,
        },
    }
}

fn test_quote(binding: &[u8; EXPORTER_BYTES]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut pcrs = Vec::new();
    pcrs.extend_from_slice(&1u32.to_le_bytes());
    pcrs.extend_from_slice(&0x000bu16.to_le_bytes());
    pcrs.push(1);
    pcrs.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
    pcrs.extend_from_slice(&[0; 5]);
    for _ in 1..8 {
        pcrs.extend_from_slice(&[0; 16]);
    }
    pcrs.extend_from_slice(&1u32.to_le_bytes());
    pcrs.extend_from_slice(&1u32.to_le_bytes());
    pcrs.extend_from_slice(&32u16.to_le_bytes());
    pcrs.extend_from_slice(&[0x42; 32]);
    pcrs.extend_from_slice(&[0; 32]);
    for _ in 1..8 {
        pcrs.extend_from_slice(&[0; 66]);
    }

    let pcr_digest = ring::digest::digest(&ring::digest::SHA256, &[0x42; 32]);
    let mut quote_message = Vec::new();
    quote_message.extend_from_slice(&0xff54_4347u32.to_be_bytes());
    quote_message.extend_from_slice(&0x8018u16.to_be_bytes());
    quote_message.extend_from_slice(&0u16.to_be_bytes());
    quote_message.extend_from_slice(&32u16.to_be_bytes());
    quote_message.extend_from_slice(binding);
    quote_message.extend_from_slice(&0u64.to_be_bytes());
    quote_message.extend_from_slice(&0u32.to_be_bytes());
    quote_message.extend_from_slice(&0u32.to_be_bytes());
    quote_message.push(1);
    quote_message.extend_from_slice(&0u64.to_be_bytes());
    quote_message.extend_from_slice(&1u32.to_be_bytes());
    quote_message.extend_from_slice(&0x000bu16.to_be_bytes());
    quote_message.push(1);
    quote_message.push(1);
    quote_message.extend_from_slice(&32u16.to_be_bytes());
    quote_message.extend_from_slice(pcr_digest.as_ref());

    let (_, private_key) =
        x509_parser::pem::parse_x509_pem(TEST_AK_PRIVATE_PEM.as_bytes()).expect("test AK PEM");
    let key = RsaKeyPair::from_pkcs8(&private_key.contents).expect("test AK PKCS#8");
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        &quote_message,
        &mut signature,
    )
    .expect("test quote signature");
    let mut quote_signature = Vec::new();
    quote_signature.extend_from_slice(&0x0014u16.to_be_bytes());
    quote_signature.extend_from_slice(&0x000bu16.to_be_bytes());
    quote_signature.extend_from_slice(&(signature.len() as u16).to_be_bytes());
    quote_signature.extend_from_slice(&signature);
    (quote_message, quote_signature, pcrs)
}

fn server_config_with_ticketer(
    certificate: CertificateDer<'static>,
    private_key: PrivateKeyDer<'static>,
    shared_ticketer: Option<Arc<dyn rustls::server::ProducesTickets>>,
    resolver_count: Option<Arc<AtomicUsize>>,
) -> ServerConfig {
    let provider = rustls::crypto::ring::default_provider();
    let signing_key = provider
        .key_provider
        .load_private_key(private_key)
        .expect("test signing key");
    let certified_key = Arc::new(CertifiedKey::new(vec![certificate], signing_key));
    let mut config = if let Some(count) = resolver_count {
        ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("test protocol versions")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(CountingCertResolver {
                key: certified_key,
                resolve_count: count,
            }))
    } else {
        ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("test protocol versions")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(StaticCertResolver(certified_key)))
    };
    if let Some(ticketer) = shared_ticketer {
        config.ticketer = ticketer;
    }
    config
}

fn certificate_without_evidence(
    shared_ticketer: Option<Arc<dyn rustls::server::ProducesTickets>>,
    resolver_count: Option<Arc<AtomicUsize>>,
) -> ServerConfig {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
    let params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
    let certificate = params.self_signed(&key).expect("certificate");
    server_config_with_ticketer(
        CertificateDer::from(certificate.der().to_vec()),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        shared_ticketer,
        resolver_count,
    )
}

fn certificate_with_evidence(
    owner_nonce: &[u8],
    shared_ticketer: Option<Arc<dyn rustls::server::ProducesTickets>>,
    resolver_count: Option<Arc<AtomicUsize>>,
) -> (ServerConfig, CompositeEvidence) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
    let params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
    let base_certificate = params.self_signed(&key).expect("base certificate");
    let (_, parsed) = x509_parser::prelude::X509Certificate::from_der(base_certificate.der())
        .expect("parse base certificate");
    let evidence = CompositeEvidence {
        owner_nonce: owner_nonce.to_vec(),
        tls_spki_der: parsed.public_key().raw.to_vec(),
        amd_report: Vec::new(),
        hcl_report: Vec::new(),
        ak_public_key_pem: TEST_AK_PUBLIC_PEM.as_bytes().to_vec(),
        quote_message: Vec::new(),
        quote_signature: Vec::new(),
        quote_pcrs: Vec::new(),
        amd_ark_pem: Vec::new(),
        amd_ask_pem: Vec::new(),
        amd_vcek_pem: Vec::new(),
        gpu_envelope: b"test GPU envelope".to_vec(),
    };
    let mut params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
    let mut extension = CustomExtension::from_oid_content(
        &[
            2,
            25,
            3_708_997_813,
            3_535_365_757,
            2_172_800_616,
            1_077_671_698,
        ],
        evidence.to_der(),
    );
    extension.set_criticality(true);
    params.custom_extensions.push(extension);
    let certificate = params.self_signed(&key).expect("evidence certificate");

    let config = server_config_with_ticketer(
        CertificateDer::from(certificate.der().to_vec()),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        shared_ticketer,
        resolver_count,
    );

    (config, evidence)
}

fn read_http_request(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "headers too large",
            ));
        }
        reader.read_exact(&mut byte)?;
        bytes.push(byte[0]);
    }
    let header = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "UTF-8 error"))?;
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let header_len = bytes.len();
    bytes.resize(header_len + content_length, 0);
    reader.read_exact(&mut bytes[header_len..])?;
    Ok(bytes)
}

fn chat_response(content: &str) -> String {
    let resp_body = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1234567890,
        "model": "qwen-test",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": content
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 5,
            "total_tokens": 10
        }
    })
    .to_string();

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{resp_body}",
        resp_body.len()
    )
}

#[derive(Default)]
struct ConnStats {
    app_requests: AtomicUsize,
}

#[derive(Default)]
struct ServerStats {
    connections_accepted: AtomicUsize,
    prefaces_read: AtomicUsize,
    proof_requests_read: AtomicUsize,
    handshake_completed: AtomicUsize,
    app_requests_read: AtomicUsize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ProofScript {
    Valid,
    Surplus,
    BareLf,
    Status503,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IdleCloseKind {
    CloseNotify,
    Fin,
    Reset,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum AppScript {
    Ok(String),
    HoldOk(String),
    SurplusSameRecord(String),
    SurplusLaterRecord(String),
    SecondResponse,
    DuplicateContentLength,
    BareLfHeader,
    TransferEncoding,
    Status(u16),
    Unparseable,
    CloseNotifyAfter(String),
    Truncated,
    PartialThenClose,
    CloseAfterHead,
    Stall,
    PartialRecordAfterReturn,
    IdleClose(IdleCloseKind),
}

#[derive(Clone)]
struct ConnScript {
    hold_handshake: bool,
    proof: ProofScript,
    app: Vec<AppScript>,
    without_evidence: bool,
}

impl Default for ConnScript {
    fn default() -> Self {
        Self {
            hold_handshake: false,
            proof: ProofScript::Valid,
            app: Vec::new(),
            without_evidence: false,
        }
    }
}

fn wait_until<F: Fn() -> bool>(cond: F) {
    let start = Instant::now();
    while !cond() {
        if start.elapsed() > Duration::from_secs(15) {
            panic!("wait_until condition timed out after 15 seconds");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

struct Gate {
    lock: Mutex<bool>,
    cvar: Condvar,
}

impl Gate {
    fn new(initial: bool) -> Self {
        Self {
            lock: Mutex::new(initial),
            cvar: Condvar::new(),
        }
    }

    fn wait(&self, shutdown: &AtomicBool) {
        let start = Instant::now();
        let mut released = self.lock.lock().unwrap();
        while !*released && !shutdown.load(Ordering::Relaxed) {
            if start.elapsed() > Duration::from_secs(15) {
                panic!("Gate wait timed out after 15 seconds");
            }
            let res = self
                .cvar
                .wait_timeout(released, Duration::from_millis(50))
                .unwrap();
            released = res.0;
        }
    }

    fn signal(&self) {
        *self.lock.lock().unwrap() = true;
        self.cvar.notify_all();
    }
}

#[derive(Default)]
struct ServerPlan {
    conn_scripts: Vec<ConnScript>,
    default_script: ConnScript,
    shared_ticketer: Option<Arc<dyn rustls::server::ProducesTickets>>,
    resolver_count: Option<Arc<AtomicUsize>>,
}

struct TestServer {
    port: u16,
    stats: Arc<ServerStats>,
    connections: Arc<Mutex<Vec<Arc<ConnStats>>>>,
    nonces: Arc<Mutex<Vec<[u8; 32]>>>,
    shutdown: Arc<AtomicBool>,
    release_handshake: Arc<Gate>,
    release_app_response: Arc<Gate>,
    inject_idle_fault: Arc<Gate>,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn spawn(default_app: AppScript) -> Self {
        let default_script = ConnScript {
            app: vec![default_app],
            ..Default::default()
        };
        Self::spawn_plan(ServerPlan {
            default_script,
            ..Default::default()
        })
    }

    fn spawn_plan(plan: ServerPlan) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        listener.set_nonblocking(true).expect("set nonblocking");

        let stats = Arc::new(ServerStats::default());
        let connections = Arc::new(Mutex::new(Vec::new()));
        let nonces = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let release_handshake = Arc::new(Gate::new(false));
        let release_app_response = Arc::new(Gate::new(false));
        let inject_idle_fault = Arc::new(Gate::new(false));

        let stats_clone = stats.clone();
        let connections_clone = connections.clone();
        let nonces_clone = nonces.clone();
        let shutdown_clone = shutdown.clone();
        let rel_handshake = release_handshake.clone();
        let rel_app = release_app_response.clone();
        let inj_fault = inject_idle_fault.clone();

        let handle = thread::spawn(move || {
            let plan = Arc::new(plan);
            let conn_index_counter = Arc::new(AtomicUsize::new(0));
            while !shutdown_clone.load(Ordering::Relaxed) {
                let (mut tcp, _) = match listener.accept() {
                    Ok(conn) => conn,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };

                let conn_stats = Arc::new(ConnStats::default());
                connections_clone.lock().unwrap().push(conn_stats.clone());
                stats_clone
                    .connections_accepted
                    .fetch_add(1, Ordering::SeqCst);

                let conn_index = conn_index_counter.fetch_add(1, Ordering::SeqCst);
                let stats_clone = stats_clone.clone();
                let nonces_clone = nonces_clone.clone();
                let shutdown_clone = shutdown_clone.clone();
                let rel_handshake = rel_handshake.clone();
                let rel_app = rel_app.clone();
                let inj_fault = inj_fault.clone();
                let plan = plan.clone();

                thread::spawn(move || {
                    // Darwin hands back a stream that inherits the listener's
                    // O_NONBLOCK; Linux never does. The server is blocking.
                    tcp.set_nonblocking(false)
                        .expect("blocking accepted stream");
                    let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
                    let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));

                    let mut preface = [0u8; 40];
                    if tcp.read_exact(&mut preface).is_err() {
                        return;
                    }
                    stats_clone.prefaces_read.fetch_add(1, Ordering::SeqCst);

                    let mut nonce = [0u8; 32];
                    nonce.copy_from_slice(&preface[8..40]);
                    nonces_clone.lock().unwrap().push(nonce);

                    let script = if conn_index < plan.conn_scripts.len() {
                        plan.conn_scripts[conn_index].clone()
                    } else {
                        plan.default_script.clone()
                    };

                    if script.hold_handshake {
                        rel_handshake.wait(&shutdown_clone);
                    }

                    let owner_nonce = &preface[8..40];
                    let (config, evidence) = if script.without_evidence {
                        (
                            certificate_without_evidence(
                                plan.shared_ticketer.clone(),
                                plan.resolver_count.clone(),
                            ),
                            CompositeEvidence {
                                owner_nonce: owner_nonce.to_vec(),
                                tls_spki_der: Vec::new(),
                                amd_report: Vec::new(),
                                hcl_report: Vec::new(),
                                ak_public_key_pem: Vec::new(),
                                quote_message: Vec::new(),
                                quote_signature: Vec::new(),
                                quote_pcrs: Vec::new(),
                                amd_ark_pem: Vec::new(),
                                amd_ask_pem: Vec::new(),
                                amd_vcek_pem: Vec::new(),
                                gpu_envelope: Vec::new(),
                            },
                        )
                    } else {
                        certificate_with_evidence(
                            owner_nonce,
                            plan.shared_ticketer.clone(),
                            plan.resolver_count.clone(),
                        )
                    };

                    let mut stream = match ServerConnection::new(Arc::new(config)) {
                        Ok(conn) => StreamOwned::new(conn, tcp),
                        Err(_) => return,
                    };

                    while stream.conn.is_handshaking() {
                        if stream.conn.complete_io(&mut stream.sock).is_err() {
                            break;
                        }
                    }
                    if stream.conn.is_handshaking() {
                        return;
                    }
                    stats_clone
                        .handshake_completed
                        .fetch_add(1, Ordering::SeqCst);

                    let proof_req = match read_http_request(&mut stream) {
                        Ok(req) => req,
                        Err(_) => return,
                    };
                    if !proof_req.starts_with(b"GET /._sol/spp/exporter-proof") {
                        return;
                    }
                    stats_clone
                        .proof_requests_read
                        .fetch_add(1, Ordering::SeqCst);

                    match script.proof {
                        ProofScript::Surplus => {
                            let proof_raw = b"bad-proof";
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n{}SURPLUS",
                                EXPORTER_PROOF_MEDIA_TYPE,
                                proof_raw.len(),
                                std::str::from_utf8(proof_raw).unwrap()
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.sock.flush();
                            return;
                        }
                        ProofScript::BareLf => {
                            let resp = format!(
                                "HTTP/1.1 200 OK\nContent-Type: {}\nContent-Length: 0\n\n",
                                EXPORTER_PROOF_MEDIA_TYPE
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.sock.flush();
                            return;
                        }
                        ProofScript::Status503 => {
                            let resp =
                                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n";
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.sock.flush();
                            return;
                        }
                        ProofScript::Valid => {
                            let mut exporter = [0; EXPORTER_BYTES];
                            if stream
                                .conn
                                .export_keying_material(
                                    &mut exporter,
                                    EXPORTER_LABEL,
                                    Some(&exporter_context(
                                        &evidence.owner_nonce,
                                        &evidence.tls_spki_der,
                                    )),
                                )
                                .is_err()
                            {
                                return;
                            }
                            let binding = exporter_binding(
                                &evidence.owner_nonce,
                                &evidence.tls_spki_der,
                                &exporter,
                                &evidence.gpu_envelope,
                            );
                            let (quote_message, quote_signature, quote_pcrs) = test_quote(&binding);
                            let proof_der = ExporterProof {
                                owner_nonce: evidence.owner_nonce,
                                tls_spki_der: evidence.tls_spki_der,
                                tls_exporter: exporter.to_vec(),
                                quote_message,
                                quote_signature,
                                quote_pcrs,
                            }
                            .to_der();

                            let proof_http = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                                EXPORTER_PROOF_MEDIA_TYPE,
                                proof_der.len()
                            )
                            .into_bytes()
                            .into_iter()
                            .chain(proof_der)
                            .collect::<Vec<u8>>();

                            if stream.write_all(&proof_http).is_err() {
                                return;
                            }
                            let _ = stream.sock.flush();
                        }
                    }

                    let mut app_index = 0;
                    while let Ok(_req) = read_http_request(&mut stream) {
                        conn_stats.app_requests.fetch_add(1, Ordering::SeqCst);
                        stats_clone.app_requests_read.fetch_add(1, Ordering::SeqCst);

                        let app_script = if app_index < script.app.len() {
                            script.app[app_index].clone()
                        } else if !script.app.is_empty() {
                            script.app.last().unwrap().clone()
                        } else {
                            AppScript::Ok("hello response".to_owned())
                        };
                        app_index += 1;

                        match app_script {
                            AppScript::HoldOk(token) => {
                                rel_app.wait(&shutdown_clone);
                                let resp = chat_response(&token);
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                            }
                            AppScript::Ok(token) => {
                                let resp = chat_response(&token);
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                            }
                            AppScript::SurplusSameRecord(token) => {
                                let mut resp = chat_response(&token).into_bytes();
                                resp.push(b'X');
                                let _ = stream.write_all(&resp);
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::SurplusLaterRecord(token) => {
                                let resp = chat_response(&token);
                                // Separate writer calls queue separate TLS records.
                                // Send both together so the anomaly is available
                                // before the next request can use the channel.
                                let _ = stream.conn.writer().write_all(resp.as_bytes());
                                let _ = stream.conn.writer().write_all(b"X");
                                let _ = stream.conn.write_tls(&mut stream.sock);
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::SecondResponse => {
                                let resp1 = chat_response("FIRST");
                                let resp2 = chat_response("LEAKED");
                                // Keep the surplus response in its own TLS record,
                                // queued before either record reaches the socket.
                                let _ = stream.conn.writer().write_all(resp1.as_bytes());
                                let _ = stream.conn.writer().write_all(resp2.as_bytes());
                                let _ = stream.conn.write_tls(&mut stream.sock);
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::DuplicateContentLength => {
                                let body = json!({
                                    "id": "chatcmpl-test",
                                    "object": "chat.completion",
                                    "created": 1234567890,
                                    "model": "qwen-test",
                                    "choices": [{
                                        "index": 0,
                                        "message": {
                                            "role": "assistant",
                                            "content": "dup"
                                        },
                                        "finish_reason": "stop"
                                    }],
                                    "usage": {"prompt_tokens": 5, "completion_tokens": 5, "total_tokens": 10}
                                }).to_string();
                                let resp = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nContent-Length: {}\r\n\r\n{body}",
                                    body.len(),
                                    body.len()
                                );
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::BareLfHeader => {
                                let resp = "HTTP/1.1 200 OK\nContent-Type: application/json\nContent-Length: 0\n\n";
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::TransferEncoding => {
                                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::Status(100) => {
                                let resp = "HTTP/1.1 100 Continue\r\n\r\n";
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::Status(code) => {
                                let resp =
                                    format!("HTTP/1.1 {code} Status\r\nContent-Length: 0\r\n\r\n");
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::Unparseable => {
                                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\n\r\nnot-json";
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::CloseNotifyAfter(token) => {
                                let resp = chat_response(&token);
                                // This fixture closes with the response. Queue both
                                // before writing to the socket so the next call cannot
                                // race a close sent after the response was returned.
                                // The after-request close is covered by oracle 9.
                                let _ = stream.conn.writer().write_all(resp.as_bytes());
                                stream.conn.send_close_notify();
                                let _ = stream.conn.write_tls(&mut stream.sock);
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::Truncated => {
                                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\nshort";
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::PartialThenClose => {
                                let resp = "HTTP/1.1 200 OK\r\n";
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::CloseAfterHead => {
                                drop(stream);
                                break;
                            }
                            AppScript::Stall => {
                                let resp = "HTTP/1.1 200 OK\r\n";
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                thread::sleep(Duration::from_secs(2));
                                break;
                            }
                            AppScript::PartialRecordAfterReturn => {
                                let resp = chat_response("clean");
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                inj_fault.wait(&shutdown_clone);
                                let _ = stream.sock.write_all(&[0x17, 0x03, 0x03]);
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::IdleClose(IdleCloseKind::CloseNotify) => {
                                let resp = chat_response("clean");
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                inj_fault.wait(&shutdown_clone);
                                stream.conn.send_close_notify();
                                let _ = stream.conn.write_tls(&mut stream.sock);
                                let _ = stream.sock.flush();
                                break;
                            }
                            AppScript::IdleClose(IdleCloseKind::Fin) => {
                                let resp = chat_response("clean");
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                inj_fault.wait(&shutdown_clone);
                                let _ = stream.sock.shutdown(std::net::Shutdown::Write);
                                break;
                            }
                            AppScript::IdleClose(IdleCloseKind::Reset) => {
                                let resp = chat_response("clean");
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.sock.flush();
                                inj_fault.wait(&shutdown_clone);
                                let _ = socket2::SockRef::from(&stream.sock)
                                    .set_linger(Some(Duration::ZERO));
                                drop(stream);
                                break;
                            }
                        }
                    }
                });
            }
        });

        Self {
            port,
            stats,
            connections,
            nonces,
            shutdown,
            release_handshake,
            release_app_response,
            inject_idle_fault,
            handle: Some(handle),
        }
    }

    fn release_handshake(&self) {
        self.release_handshake.signal();
    }

    fn release_app_response(&self) {
        self.release_app_response.signal();
    }

    fn inject_idle_fault(&self) {
        self.inject_idle_fault.signal();
    }

    fn connection(&self, index: usize) -> Arc<ConnStats> {
        self.connections.lock().unwrap()[index].clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.release_handshake.signal();
        self.release_app_response.signal();
        self.inject_idle_fault.signal();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn test_endpoint(port: u16) -> ByoEndpoint {
    ByoEndpoint {
        base_url: format!("http://127.0.0.1:{port}"),
        served_model_id: "qwen-test".into(),
        credential: Some("token".into()),
        parallel_slots: None,
        is_bundled: false,
        is_confidential: true,
    }
}

fn request(id: &str) -> GenerateRequest {
    GenerateRequest {
        id: Some(id.to_owned()),
        context: "test.confidential".to_owned(),
        contents: vec![ContentPart::Text {
            text: "hello request".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.2,
        max_output_tokens: 64,
        timeout_s: Some(3.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn temp_journal(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solstone-confidential-reuse-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&path);
    path
}

fn reason_code(result: &ConfidentialResult) -> Option<&str> {
    match result {
        ConfidentialResult::Failed(error) => error.reason_code.as_deref(),
        ConfidentialResult::AttestationFailed(reason) => Some(reason),
        ConfidentialResult::AttestationNotVerified => Some("attestation_not_verified"),
        _ => None,
    }
}

struct MockClock {
    system: Mutex<SystemTime>,
    monotonic: Mutex<Instant>,
}

impl MockClock {
    fn new() -> Self {
        Self {
            system: Mutex::new(SystemTime::now()),
            monotonic: Mutex::new(Instant::now()),
        }
    }

    fn advance_both(&self, duration: Duration) {
        *self.system.lock().unwrap() += duration;
        *self.monotonic.lock().unwrap() += duration;
    }

    fn advance_system(&self, duration: Duration) {
        *self.system.lock().unwrap() += duration;
    }

    fn set_system(&self, time: SystemTime) {
        *self.system.lock().unwrap() = time;
    }
}

impl PoolClock for MockClock {
    fn now_system(&self) -> SystemTime {
        *self.system.lock().unwrap()
    }
    fn now_monotonic(&self) -> Instant {
        *self.monotonic.lock().unwrap()
    }
}

#[test]
fn oracle_1_sequential_reuse_and_one_shot_runtimes() {
    let server1 = TestServer::spawn(AppScript::Ok("hello response".to_owned()));
    let runtime1 = EndpointRuntime::new(1);
    let journal_path1 = temp_journal("oracle_1_seq");
    let endpoint1 = test_endpoint(server1.port);
    let config = Map::new();

    // 3 sequential calls on runtime1
    for i in 1..=3 {
        let res = confidential_generate_attested(
            &request(&format!("req-{i}")),
            &journal_path1,
            &endpoint1,
            &config,
            &runtime1,
            &AcceptingCompositeVerifier,
        );
        let ConfidentialResult::Generated(gen_resp) = res else {
            panic!("expected generated");
        };
        assert_eq!(gen_resp.text, "hello response");
    }

    assert_eq!(server1.stats.prefaces_read.load(Ordering::SeqCst), 1);
    assert_eq!(server1.stats.proof_requests_read.load(Ordering::SeqCst), 1);
    assert_eq!(server1.stats.app_requests_read.load(Ordering::SeqCst), 3);

    let nonces1: HashSet<_> = server1.nonces.lock().unwrap().iter().cloned().collect();
    assert_eq!(nonces1.len(), 1);
    let _ = fs::remove_dir_all(&journal_path1);

    // New server and 3 separate EndpointRuntime::default() calls
    let server2 = TestServer::spawn(AppScript::Ok("hello response".to_owned()));
    let endpoint2 = test_endpoint(server2.port);
    let journal_path2 = temp_journal("oracle_1_oneshot");

    for i in 1..=3 {
        let rt = EndpointRuntime::default();
        let res = confidential_generate_attested(
            &request(&format!("req-oneshot-{i}")),
            &journal_path2,
            &endpoint2,
            &config,
            &rt,
            &AcceptingCompositeVerifier,
        );
        let ConfidentialResult::Generated(gen_resp) = res else {
            panic!("expected generated");
        };
        assert_eq!(gen_resp.text, "hello response");
    }

    assert_eq!(server2.stats.prefaces_read.load(Ordering::SeqCst), 3);
    let nonces2: HashSet<_> = server2.nonces.lock().unwrap().iter().cloned().collect();
    assert_eq!(nonces2.len(), 3);
    let _ = fs::remove_dir_all(&journal_path2);
}

#[test]
fn oracle_2_live_channels_capped_by_workers() {
    let server = TestServer::spawn_plan(ServerPlan {
        default_script: ConnScript {
            hold_handshake: true,
            proof: ProofScript::Valid,
            app: vec![AppScript::Ok("hello response".to_owned())],
            without_evidence: false,
        },
        ..Default::default()
    });
    let runtime = Arc::new(EndpointRuntime::new(3));
    let journal_path = temp_journal("oracle_2");
    let endpoint = test_endpoint(server.port);

    // Start 3 threads
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let rt = runtime.clone();
            let ep = endpoint.clone();
            let jp = journal_path.clone();
            thread::spawn(move || {
                confidential_generate_attested(
                    &request(&format!("req-parallel-{i}")),
                    &jp,
                    &ep,
                    &Map::new(),
                    &rt,
                    &AcceptingCompositeVerifier,
                )
            })
        })
        .collect();

    // Wait until prefaces == 3
    wait_until(|| server.stats.prefaces_read.load(Ordering::SeqCst) >= 3);

    assert_eq!(server.stats.handshake_completed.load(Ordering::SeqCst), 0);
    assert_eq!(server.stats.connections_accepted.load(Ordering::SeqCst), 3);

    // 4th call returns local_capacity_exhausted and connections is still 3
    let res4 = confidential_generate_attested(
        &request("req-4"),
        &journal_path,
        &endpoint,
        &Map::new(),
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res4), Some("local_capacity_exhausted"));
    assert_eq!(server.stats.connections_accepted.load(Ordering::SeqCst), 3);

    // Release handshake
    server.release_handshake();

    for h in handles {
        let r = h.join().unwrap();
        assert!(matches!(r, ConfidentialResult::Generated(_)));
    }
    assert_eq!(server.stats.connections_accepted.load(Ordering::SeqCst), 3);

    // Second wave of 3 concurrent calls reuses connections
    let handles2: Vec<_> = (0..3)
        .map(|i| {
            let rt = runtime.clone();
            let ep = endpoint.clone();
            let jp = journal_path.clone();
            thread::spawn(move || {
                confidential_generate_attested(
                    &request(&format!("req-wave2-{i}")),
                    &jp,
                    &ep,
                    &Map::new(),
                    &rt,
                    &AcceptingCompositeVerifier,
                )
            })
        })
        .collect();

    for h in handles2 {
        let r = h.join().unwrap();
        assert!(matches!(r, ConfidentialResult::Generated(_)));
    }

    assert_eq!(server.stats.connections_accepted.load(Ordering::SeqCst), 3);
    assert_eq!(server.stats.app_requests_read.load(Ordering::SeqCst), 6);
    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_4_key_change_establishes_fresh() {
    let config = Map::new();

    // 1. Credential change
    let server_cred = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let runtime_cred = EndpointRuntime::new(2);
    let journal_cred = temp_journal("oracle_4_cred");
    let mut ep_cred1 = test_endpoint(server_cred.port);
    ep_cred1.credential = Some("token".into());

    let res1 = confidential_generate_attested(
        &request("req-cred1"),
        &journal_cred,
        &ep_cred1,
        &config,
        &runtime_cred,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));
    let conn0 = server_cred.connection(0);

    let mut ep_cred2 = test_endpoint(server_cred.port);
    ep_cred2.credential = Some("other".into());
    let res2 = confidential_generate_attested(
        &request("req-cred2"),
        &journal_cred,
        &ep_cred2,
        &config,
        &runtime_cred,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(conn0.app_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server_cred.stats.prefaces_read.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(&journal_cred);

    // 2. Journal directory change
    let server_j = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let runtime_j = EndpointRuntime::new(2);
    let journal_j1 = temp_journal("oracle_4_j1");
    let journal_j2 = temp_journal("oracle_4_j2");
    let ep_j = test_endpoint(server_j.port);

    let res1 = confidential_generate_attested(
        &request("req-j1"),
        &journal_j1,
        &ep_j,
        &config,
        &runtime_j,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));
    let conn0 = server_j.connection(0);

    let res2 = confidential_generate_attested(
        &request("req-j2"),
        &journal_j2,
        &ep_j,
        &config,
        &runtime_j,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(conn0.app_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server_j.stats.prefaces_read.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(&journal_j1);
    let _ = fs::remove_dir_all(&journal_j2);

    // 3. Config nvattest_dir change
    let server_cfg = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let runtime_cfg = EndpointRuntime::new(2);
    let journal_cfg = temp_journal("oracle_4_cfg");
    let ep_cfg = test_endpoint(server_cfg.port);

    let res1 = confidential_generate_attested(
        &request("req-cfg1"),
        &journal_cfg,
        &ep_cfg,
        &Map::new(),
        &runtime_cfg,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));
    let conn0 = server_cfg.connection(0);

    let mut changed_config = Map::new();
    changed_config.insert(
        "services".to_owned(),
        json!({"confidential": {"nvattest_dir": "/tmp/custom-nvattest-dir"}}),
    );
    let res2 = confidential_generate_attested(
        &request("req-cfg2"),
        &journal_cfg,
        &ep_cfg,
        &changed_config,
        &runtime_cfg,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(conn0.app_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server_cfg.stats.prefaces_read.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(&journal_cfg);

    // 4. Port change
    let server_p1 = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let server_p2 = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let runtime_p = EndpointRuntime::new(2);
    let journal_p = temp_journal("oracle_4_port");
    let ep_p1 = test_endpoint(server_p1.port);
    let ep_p2 = test_endpoint(server_p2.port);

    let res1 = confidential_generate_attested(
        &request("req-p1"),
        &journal_p,
        &ep_p1,
        &config,
        &runtime_p,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));
    let conn0 = server_p1.connection(0);

    let res2 = confidential_generate_attested(
        &request("req-p2"),
        &journal_p,
        &ep_p2,
        &config,
        &runtime_p,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(conn0.app_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server_p1.stats.prefaces_read.load(Ordering::SeqCst), 1);
    assert_eq!(server_p2.stats.prefaces_read.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&journal_p);
}

#[test]
fn an_idle_channel_under_another_key_never_refuses_a_fresh_one() {
    // Limit 1 is an import session: its one idle channel must give way when the
    // credential renews, rather than refusing every call until it ages out.
    let config = Map::new();
    let server = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let runtime = EndpointRuntime::new(1);
    let journal = temp_journal("idle_never_refuses");
    let mut first = test_endpoint(server.port);
    first.credential = Some("token".into());
    let res1 = confidential_generate_attested(
        &request("req-a"),
        &journal,
        &first,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));

    let mut renewed = test_endpoint(server.port);
    renewed.credential = Some("renewed".into());
    for id in ["req-b", "req-c"] {
        let result = confidential_generate_attested(
            &request(id),
            &journal,
            &renewed,
            &config,
            &runtime,
            &AcceptingCompositeVerifier,
        );
        assert!(
            matches!(result, ConfidentialResult::Generated(_)),
            "{:?}",
            reason_code(&result)
        );
    }
    // One channel for the old credential, one reused for both renewed calls.
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 2);
    assert_eq!(server.connection(0).app_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server.connection(1).app_requests.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(&journal);
}

#[test]
fn oracle_5_epoch_drops_idle_channels() {
    let journal_path = temp_journal("oracle_5");
    let config = Map::new();

    // 1. Warm channel, second connection uses proof Status503 -> epoch increased -> fresh channel
    let server1 = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: vec![AppScript::HoldOk("warm".to_owned())],
                without_evidence: false,
            },
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Status503,
                app: Vec::new(),
                without_evidence: false,
            },
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("fresh".to_owned())],
                without_evidence: false,
            },
        ],
        ..Default::default()
    });
    let runtime1 = Arc::new(EndpointRuntime::new(2));
    let ep1 = test_endpoint(server1.port);

    let rt1_a = runtime1.clone();
    let ep1_a = ep1.clone();
    let jp1_a = journal_path.clone();
    let handle1_a = thread::spawn(move || {
        confidential_generate_attested(
            &request("req-1"),
            &jp1_a,
            &ep1_a,
            &Map::new(),
            &rt1_a,
            &AcceptingCompositeVerifier,
        )
    });

    wait_until(|| {
        !server1.connections.lock().unwrap().is_empty()
            && server1.connection(0).app_requests.load(Ordering::SeqCst) >= 1
    });

    let epoch_before1 = runtime1.confidential_channel_pool().epoch();
    let res_fail1 = confidential_generate_attested(
        &request("req-fail"),
        &journal_path,
        &ep1,
        &config,
        &runtime1,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res_fail1), Some("proof_http_failed"));
    let epoch_after1 = runtime1.confidential_channel_pool().epoch();
    assert!(epoch_after1 > epoch_before1);

    server1.release_app_response();
    let res1_a = handle1_a.join().unwrap();
    assert!(matches!(res1_a, ConfidentialResult::Generated(_)));

    let res_fresh1 = confidential_generate_attested(
        &request("req-fresh"),
        &journal_path,
        &ep1,
        &config,
        &runtime1,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_fresh1, ConfidentialResult::Generated(_)));
    assert_eq!(server1.stats.prefaces_read.load(Ordering::SeqCst), 3);

    // 2. Warm channel, next establishment uses certificate_without_evidence -> certificate_extension_missing -> epoch increased
    let server2 = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: vec![AppScript::HoldOk("warm".to_owned())],
                without_evidence: false,
            },
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: Vec::new(),
                without_evidence: true,
            },
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("fresh".to_owned())],
                without_evidence: false,
            },
        ],
        ..Default::default()
    });
    let runtime2 = Arc::new(EndpointRuntime::new(2));
    let ep2 = test_endpoint(server2.port);

    let rt2_a = runtime2.clone();
    let ep2_a = ep2.clone();
    let jp2_a = journal_path.clone();
    let handle2_a = thread::spawn(move || {
        confidential_generate_attested(
            &request("req-1"),
            &jp2_a,
            &ep2_a,
            &Map::new(),
            &rt2_a,
            &AcceptingCompositeVerifier,
        )
    });

    wait_until(|| {
        !server2.connections.lock().unwrap().is_empty()
            && server2.connection(0).app_requests.load(Ordering::SeqCst) >= 1
    });

    let epoch_before2 = runtime2.confidential_channel_pool().epoch();
    let res_fail2 = confidential_generate_attested(
        &request("req-no-ev"),
        &journal_path,
        &ep2,
        &config,
        &runtime2,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(
        reason_code(&res_fail2),
        Some("certificate_extension_missing")
    );
    assert!(runtime2.confidential_channel_pool().epoch() > epoch_before2);

    server2.release_app_response();
    let res2_a = handle2_a.join().unwrap();
    assert!(matches!(res2_a, ConfidentialResult::Generated(_)));

    let res_fresh2 = confidential_generate_attested(
        &request("req-fresh"),
        &journal_path,
        &ep2,
        &config,
        &runtime2,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_fresh2, ConfidentialResult::Generated(_)));
    assert_eq!(server2.stats.prefaces_read.load(Ordering::SeqCst), 3);

    // 3. Warm channel, next call passes RejectingCompositeVerifier -> composite_appraisal_failed -> epoch increased, app count 0
    let server3 = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: vec![AppScript::HoldOk("warm".to_owned())],
                without_evidence: false,
            },
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("never-reached".to_owned())],
                without_evidence: false,
            },
            ConnScript {
                hold_handshake: false,
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("fresh".to_owned())],
                without_evidence: false,
            },
        ],
        ..Default::default()
    });
    let runtime3 = Arc::new(EndpointRuntime::new(2));
    let ep3 = test_endpoint(server3.port);

    let rt3_a = runtime3.clone();
    let ep3_a = ep3.clone();
    let jp3_a = journal_path.clone();
    let handle3_a = thread::spawn(move || {
        confidential_generate_attested(
            &request("req-1"),
            &jp3_a,
            &ep3_a,
            &Map::new(),
            &rt3_a,
            &AcceptingCompositeVerifier,
        )
    });

    wait_until(|| {
        !server3.connections.lock().unwrap().is_empty()
            && server3.connection(0).app_requests.load(Ordering::SeqCst) >= 1
    });

    let epoch_before3 = runtime3.confidential_channel_pool().epoch();
    let res_reject3 = confidential_generate_attested(
        &request("req-reject"),
        &journal_path,
        &ep3,
        &config,
        &runtime3,
        &RejectingCompositeVerifier,
    );
    assert_eq!(
        reason_code(&res_reject3),
        Some("composite_appraisal_failed")
    );
    assert!(runtime3.confidential_channel_pool().epoch() > epoch_before3);
    assert_eq!(server3.connection(1).app_requests.load(Ordering::SeqCst), 0);

    server3.release_app_response();
    let res3_a = handle3_a.join().unwrap();
    assert!(matches!(res3_a, ConfidentialResult::Generated(_)));

    let res_fresh3 = confidential_generate_attested(
        &request("req-fresh"),
        &journal_path,
        &ep3,
        &config,
        &runtime3,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_fresh3, ConfidentialResult::Generated(_)));
    assert_eq!(server3.stats.prefaces_read.load(Ordering::SeqCst), 3);

    // 4. Warm channel, port 1 unreachable -> gateway_unreachable -> epoch unchanged, following call reuses
    let server4 = TestServer::spawn(AppScript::Ok("warm".to_owned()));
    let runtime4 = EndpointRuntime::new(2);
    let ep4 = test_endpoint(server4.port);
    let mut ep_unreachable = test_endpoint(1);
    ep_unreachable.base_url = "http://127.0.0.1:1".into();

    let _ = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &ep4,
        &config,
        &runtime4,
        &AcceptingCompositeVerifier,
    );
    let epoch_before = runtime4.confidential_channel_pool().epoch();

    let res_unreach = confidential_generate_attested(
        &request("req-unreach"),
        &journal_path,
        &ep_unreachable,
        &config,
        &runtime4,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res_unreach), Some("gateway_unreachable"));
    assert_eq!(runtime4.confidential_channel_pool().epoch(), epoch_before);

    let res_reuse = confidential_generate_attested(
        &request("req-reuse"),
        &journal_path,
        &ep4,
        &config,
        &runtime4,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_reuse, ConfidentialResult::Generated(_)));
    assert_eq!(server4.stats.prefaces_read.load(Ordering::SeqCst), 1);
    assert_eq!(server4.connection(0).app_requests.load(Ordering::SeqCst), 2);

    // 5. Hold worker A's app response, worker B proof Status503 bumps epoch -> A returns token-a -> next call prefaces
    let server5 = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![
            ConnScript {
                proof: ProofScript::Valid,
                app: vec![AppScript::HoldOk("token-a".to_owned())],
                ..Default::default()
            },
            ConnScript {
                proof: ProofScript::Status503,
                ..Default::default()
            },
            ConnScript {
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("token-c".to_owned())],
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let runtime5 = Arc::new(EndpointRuntime::new(2));
    let ep5 = test_endpoint(server5.port);

    let rt_a = runtime5.clone();
    let ep_a = ep5.clone();
    let jp_a = journal_path.clone();
    let handle_a = thread::spawn(move || {
        confidential_generate_attested(
            &request("req-a"),
            &jp_a,
            &ep_a,
            &Map::new(),
            &rt_a,
            &AcceptingCompositeVerifier,
        )
    });

    // Wait until A's socket app count is 1
    wait_until(|| {
        !server5.connections.lock().unwrap().is_empty()
            && server5.connection(0).app_requests.load(Ordering::SeqCst) >= 1
    });

    let epoch_before_b = runtime5.confidential_channel_pool().epoch();
    let res_b = confidential_generate_attested(
        &request("req-b"),
        &journal_path,
        &ep5,
        &config,
        &runtime5,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res_b), Some("proof_http_failed"));
    assert!(runtime5.confidential_channel_pool().epoch() > epoch_before_b);

    // Release A
    server5.release_app_response();
    let res_a = handle_a.join().unwrap();
    let ConfidentialResult::Generated(gen_a) = res_a else {
        panic!("expected A generated");
    };
    assert_eq!(gen_a.text, "token-a");

    // Next call prefaces because A was not returned to pool (epoch bumped while in flight)
    let res_after = confidential_generate_attested(
        &request("req-after"),
        &journal_path,
        &ep5,
        &config,
        &runtime5,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_after, ConfidentialResult::Generated(_)));
    assert_eq!(server5.stats.prefaces_read.load(Ordering::SeqCst), 3);

    // 6. Hold A's handshake, B's proof Status503 bumps epoch -> release A -> A is Generated with token-a -> next call prefaces
    let server6 = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![
            ConnScript {
                hold_handshake: true,
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("token-a6".to_owned())],
                ..Default::default()
            },
            ConnScript {
                proof: ProofScript::Status503,
                ..Default::default()
            },
            ConnScript {
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("token-c6".to_owned())],
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let runtime6 = Arc::new(EndpointRuntime::new(2));
    let ep6 = test_endpoint(server6.port);

    let rt_a6 = runtime6.clone();
    let ep_a6 = ep6.clone();
    let jp_a6 = journal_path.clone();
    let handle_a6 = thread::spawn(move || {
        confidential_generate_attested(
            &request("req-a6"),
            &jp_a6,
            &ep_a6,
            &Map::new(),
            &rt_a6,
            &AcceptingCompositeVerifier,
        )
    });

    wait_until(|| server6.stats.prefaces_read.load(Ordering::SeqCst) >= 1);
    assert_eq!(server6.stats.handshake_completed.load(Ordering::SeqCst), 0);

    let epoch_before_b6 = runtime6.confidential_channel_pool().epoch();
    let res_b6 = confidential_generate_attested(
        &request("req-b6"),
        &journal_path,
        &ep6,
        &config,
        &runtime6,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res_b6), Some("proof_http_failed"));
    assert!(runtime6.confidential_channel_pool().epoch() > epoch_before_b6);

    server6.release_handshake();
    let res_a6 = handle_a6.join().unwrap();
    let ConfidentialResult::Generated(gen_a6) = res_a6 else {
        panic!("expected A6 generated");
    };
    assert_eq!(gen_a6.text, "token-a6");
    assert_eq!(server6.connection(0).app_requests.load(Ordering::SeqCst), 1);

    let res_after6 = confidential_generate_attested(
        &request("req-after6"),
        &journal_path,
        &ep6,
        &config,
        &runtime6,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_after6, ConfidentialResult::Generated(_)));
    assert_eq!(server6.stats.prefaces_read.load(Ordering::SeqCst), 3);

    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_6_age_on_both_clocks() {
    let journal_path = temp_journal("oracle_6");
    let config = Map::new();

    // 1. advance_both(119s): second call does not preface
    let server1 = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let mock_clock1 = Arc::new(MockClock::new());
    let runtime1 = EndpointRuntime::with_pool_clock(1, mock_clock1.clone());
    let ep1 = test_endpoint(server1.port);

    let _ = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &ep1,
        &config,
        &runtime1,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(server1.stats.prefaces_read.load(Ordering::SeqCst), 1);

    mock_clock1.advance_both(Duration::from_secs(119));

    let res2 = confidential_generate_attested(
        &request("req-2"),
        &journal_path,
        &ep1,
        &config,
        &runtime1,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(server1.stats.prefaces_read.load(Ordering::SeqCst), 1);

    // 2. advance_both(121s) on a fresh server: second call prefaces
    let server2 = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let mock_clock2 = Arc::new(MockClock::new());
    let runtime2 = EndpointRuntime::with_pool_clock(1, mock_clock2.clone());
    let ep2 = test_endpoint(server2.port);

    let _ = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &ep2,
        &config,
        &runtime2,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(server2.stats.prefaces_read.load(Ordering::SeqCst), 1);

    mock_clock2.advance_both(Duration::from_secs(121));

    let res2 = confidential_generate_attested(
        &request("req-2"),
        &journal_path,
        &ep2,
        &config,
        &runtime2,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(server2.stats.prefaces_read.load(Ordering::SeqCst), 2);

    // 3. Fresh server: set_system to UNIX_EPOCH, monotonic unchanged -> second call prefaces
    let server3 = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let mock_clock3 = Arc::new(MockClock::new());
    let runtime3 = EndpointRuntime::with_pool_clock(1, mock_clock3.clone());
    let ep3 = test_endpoint(server3.port);

    let _ = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &ep3,
        &config,
        &runtime3,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(server3.stats.prefaces_read.load(Ordering::SeqCst), 1);

    mock_clock3.set_system(UNIX_EPOCH);

    let res2 = confidential_generate_attested(
        &request("req-2"),
        &journal_path,
        &ep3,
        &config,
        &runtime3,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(server3.stats.prefaces_read.load(Ordering::SeqCst), 2);

    // 4. Fresh server: advance_system(600s), monotonic unchanged -> second call prefaces
    let server4 = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let mock_clock4 = Arc::new(MockClock::new());
    let runtime4 = EndpointRuntime::with_pool_clock(1, mock_clock4.clone());
    let ep4 = test_endpoint(server4.port);

    let _ = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &ep4,
        &config,
        &runtime4,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(server4.stats.prefaces_read.load(Ordering::SeqCst), 1);

    mock_clock4.advance_system(Duration::from_secs(600));

    let res2 = confidential_generate_attested(
        &request("req-2"),
        &journal_path,
        &ep4,
        &config,
        &runtime4,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));
    assert_eq!(server4.stats.prefaces_read.load(Ordering::SeqCst), 2);

    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_7_health_probe_is_never_pooled() {
    let server = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let runtime = EndpointRuntime::new(2);
    let journal_path = temp_journal("oracle_7");
    let endpoint = test_endpoint(server.port);
    let config = Map::new();

    // 1. One normal call
    let res1 = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 1);

    // 2. Two health probe requests
    let mut probe_req1 = request("req-probe-1");
    probe_req1.context = HEALTH_BRAIN_GENERATE_CONTEXT.to_owned();
    let res_probe1 = confidential_generate_attested(
        &probe_req1,
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_probe1, ConfidentialResult::Generated(_)));

    let mut probe_req2 = request("req-probe-2");
    probe_req2.context = HEALTH_BRAIN_GENERATE_CONTEXT.to_owned();
    let res_probe2 = confidential_generate_attested(
        &probe_req2,
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_probe2, ConfidentialResult::Generated(_)));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 3);

    // 3. One more normal call does not preface (reuses the initial pooled channel)
    let res_normal = confidential_generate_attested(
        &request("req-normal"),
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_normal, ConfidentialResult::Generated(_)));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 3);

    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_8_framing_does_not_reuse() {
    let journal_path = temp_journal("oracle_8");
    let config = Map::new();

    let invalid_scripts = vec![
        AppScript::SurplusSameRecord("first-token".to_owned()),
        AppScript::SurplusLaterRecord("first-token".to_owned()),
        AppScript::SecondResponse,
        AppScript::DuplicateContentLength,
        AppScript::BareLfHeader,
        AppScript::TransferEncoding,
        AppScript::Status(100),
        AppScript::Status(204),
        AppScript::Status(401),
        AppScript::Status(503),
        AppScript::Unparseable,
        AppScript::Truncated,
        AppScript::PartialThenClose,
        AppScript::CloseAfterHead,
        AppScript::Stall,
    ];

    for script in invalid_scripts {
        let is_stall = matches!(script, AppScript::Stall);
        let server = TestServer::spawn_plan(ServerPlan {
            conn_scripts: vec![
                ConnScript {
                    proof: ProofScript::Valid,
                    app: vec![script.clone()],
                    ..Default::default()
                },
                ConnScript {
                    proof: ProofScript::Valid,
                    app: vec![AppScript::Ok("second-token".to_owned())],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let runtime = EndpointRuntime::default();
        let endpoint = test_endpoint(server.port);

        let mut req1 = request("req-1");
        if is_stall {
            req1.timeout_s = Some(1.0);
        }

        let res1 = confidential_generate_attested(
            &req1,
            &journal_path,
            &endpoint,
            &config,
            &runtime,
            &AcceptingCompositeVerifier,
        );
        let code1 = reason_code(&res1);
        // Surplus TLS records are queued with the complete response. Depending
        // on the client's read boundary it may return that first response or
        // reject its framing; the next call must use a fresh channel either way.
        let arrives_later = matches!(
            script,
            AppScript::SurplusLaterRecord(_) | AppScript::SecondResponse
        );
        if arrives_later {
            assert!(
                code1 == Some("provider_response_invalid")
                    || matches!(&res1, ConfidentialResult::Generated(generated) if generated.text == "first-token" || generated.text == "FIRST"),
                "{script:?}: {code1:?}"
            );
        } else if is_stall {
            assert!(
                code1 == Some("local_capacity_exhausted")
                    || code1 == Some("provider_response_invalid")
            );
        } else {
            assert_eq!(code1, Some("provider_response_invalid"));
        }
        assert_ne!(code1, Some("confidential_channel_closed"));

        let res2 = confidential_generate_attested(
            &request("req-2"),
            &journal_path,
            &endpoint,
            &config,
            &runtime,
            &AcceptingCompositeVerifier,
        );
        let ConfidentialResult::Generated(gen2) = res2 else {
            panic!(
                "expected second generated for script {:?}: {:?}",
                script,
                reason_code(&res2)
            );
        };
        assert_eq!(gen2.text, "second-token");
        assert_ne!(gen2.text, "LEAKED");
        assert_ne!(gen2.text, "first-token");
        assert_eq!(server.stats.connections_accepted.load(Ordering::SeqCst), 2);
    }

    // CloseNotifyAfter: first call generated, second call still prefaces
    let server_cn = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![
            ConnScript {
                proof: ProofScript::Valid,
                app: vec![AppScript::CloseNotifyAfter("first-token".to_owned())],
                ..Default::default()
            },
            ConnScript {
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("second-token".to_owned())],
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let runtime_cn = EndpointRuntime::default();
    let ep_cn = test_endpoint(server_cn.port);

    let res_cn1 = confidential_generate_attested(
        &request("req-cn1"),
        &journal_path,
        &ep_cn,
        &config,
        &runtime_cn,
        &AcceptingCompositeVerifier,
    );
    if let ConfidentialResult::Generated(gen_cn1) = &res_cn1 {
        assert_eq!(gen_cn1.text, "first-token");
    }

    let res_cn2 = confidential_generate_attested(
        &request("req-cn2"),
        &journal_path,
        &ep_cn,
        &config,
        &runtime_cn,
        &AcceptingCompositeVerifier,
    );
    let ConfidentialResult::Generated(gen_cn2) = res_cn2 else {
        panic!("expected cn2 generated: {:?}", reason_code(&res_cn2));
    };
    assert_eq!(gen_cn2.text, "second-token");
    assert_eq!(
        server_cn.stats.connections_accepted.load(Ordering::SeqCst),
        2
    );

    // PartialRecordAfterReturn: first call Generated, inject fault, second call prefaces
    let server_part = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![
            ConnScript {
                proof: ProofScript::Valid,
                app: vec![AppScript::PartialRecordAfterReturn],
                ..Default::default()
            },
            ConnScript {
                proof: ProofScript::Valid,
                app: vec![AppScript::Ok("second-token".to_owned())],
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let runtime_part = EndpointRuntime::default();
    let ep_part = test_endpoint(server_part.port);

    let res_p1 = confidential_generate_attested(
        &request("req-p1"),
        &journal_path,
        &ep_part,
        &config,
        &runtime_part,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_p1, ConfidentialResult::Generated(_)));

    server_part.inject_idle_fault();
    thread::sleep(Duration::from_millis(50));

    let res_p2 = confidential_generate_attested(
        &request("req-p2"),
        &journal_path,
        &ep_part,
        &config,
        &runtime_part,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_p2, ConfidentialResult::Generated(_)));
    assert_eq!(
        server_part
            .stats
            .connections_accepted
            .load(Ordering::SeqCst),
        2
    );
    assert_eq!(
        server_part
            .connection(0)
            .app_requests
            .load(Ordering::SeqCst),
        1
    );

    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_9_close_before_and_after_the_request() {
    let journal_path = temp_journal("oracle_9");
    let config = Map::new();

    // 1. Three idle faults (CloseNotify, Fin, Reset)
    for fault in [
        IdleCloseKind::CloseNotify,
        IdleCloseKind::Fin,
        IdleCloseKind::Reset,
    ] {
        let server = TestServer::spawn_plan(ServerPlan {
            conn_scripts: vec![
                ConnScript {
                    proof: ProofScript::Valid,
                    app: vec![AppScript::IdleClose(fault)],
                    ..Default::default()
                },
                ConnScript {
                    proof: ProofScript::Valid,
                    app: vec![AppScript::Ok("resp-2".to_owned())],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let runtime = EndpointRuntime::default();
        let endpoint = test_endpoint(server.port);

        let res1 = confidential_generate_attested(
            &request("req-1"),
            &journal_path,
            &endpoint,
            &config,
            &runtime,
            &AcceptingCompositeVerifier,
        );
        assert!(matches!(res1, ConfidentialResult::Generated(_)));

        server.inject_idle_fault();
        thread::sleep(Duration::from_millis(50));

        let res2 = confidential_generate_attested(
            &request("req-2"),
            &journal_path,
            &endpoint,
            &config,
            &runtime,
            &AcceptingCompositeVerifier,
        );
        assert!(matches!(res2, ConfidentialResult::Generated(_)));
        assert_eq!(server.stats.connections_accepted.load(Ordering::SeqCst), 2);
        assert_eq!(server.connection(0).app_requests.load(Ordering::SeqCst), 1);
        assert_eq!(server.connection(1).app_requests.load(Ordering::SeqCst), 1);
    }

    // 2. CloseAfterHead on reused channel -> confidential_channel_closed
    let server_head = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![ConnScript {
            proof: ProofScript::Valid,
            app: vec![
                AppScript::Ok("resp-1".to_owned()),
                AppScript::CloseAfterHead,
            ],
            ..Default::default()
        }],
        ..Default::default()
    });
    let runtime_head = EndpointRuntime::default();
    let ep_head = test_endpoint(server_head.port);

    let res_warm = confidential_generate_attested(
        &request("req-warm"),
        &journal_path,
        &ep_head,
        &config,
        &runtime_head,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_warm, ConfidentialResult::Generated(_)));

    let res_closed = confidential_generate_attested(
        &request("req-closed"),
        &journal_path,
        &ep_head,
        &config,
        &runtime_head,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(
        reason_code(&res_closed),
        Some("confidential_channel_closed")
    );
    assert_eq!(
        server_head
            .connection(0)
            .app_requests
            .load(Ordering::SeqCst),
        2
    );

    // 3. CloseAfterHead on fresh runtime -> provider_response_invalid or local_endpoint_unreachable, not confidential_channel_closed
    let server_fresh = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![ConnScript {
            proof: ProofScript::Valid,
            app: vec![AppScript::CloseAfterHead],
            ..Default::default()
        }],
        ..Default::default()
    });
    let runtime_fresh = EndpointRuntime::default();
    let ep_fresh = test_endpoint(server_fresh.port);

    let res_fresh = confidential_generate_attested(
        &request("req-fresh"),
        &journal_path,
        &ep_fresh,
        &config,
        &runtime_fresh,
        &AcceptingCompositeVerifier,
    );
    let code_fresh = reason_code(&res_fresh);
    assert!(
        code_fresh == Some("provider_response_invalid")
            || code_fresh == Some("local_endpoint_unreachable")
    );
    assert_ne!(code_fresh, Some("confidential_channel_closed"));

    // 4. PartialThenClose on reused channel -> not confidential_channel_closed
    let server_part = TestServer::spawn_plan(ServerPlan {
        conn_scripts: vec![ConnScript {
            proof: ProofScript::Valid,
            app: vec![
                AppScript::Ok("resp-1".to_owned()),
                AppScript::PartialThenClose,
            ],
            ..Default::default()
        }],
        ..Default::default()
    });
    let runtime_part = EndpointRuntime::default();
    let ep_part = test_endpoint(server_part.port);

    let _ = confidential_generate_attested(
        &request("req-warm"),
        &journal_path,
        &ep_part,
        &config,
        &runtime_part,
        &AcceptingCompositeVerifier,
    );
    let res_part2 = confidential_generate_attested(
        &request("req-part2"),
        &journal_path,
        &ep_part,
        &config,
        &runtime_part,
        &AcceptingCompositeVerifier,
    );
    let code_part2 = reason_code(&res_part2);
    assert_ne!(code_part2, Some("confidential_channel_closed"));

    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_10_no_bytes_on_refusal_and_drain() {
    let server = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let runtime = EndpointRuntime::default();
    let journal_path = temp_journal("oracle_10");
    let endpoint = test_endpoint(server.port);
    let config = Map::new();

    // 1. One warm call
    let res1 = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.confidential_channel_pool().epoch(), 0);

    // 2. NvattestEnsureStatus::Unavailable -> AttestationNotVerified, prefaces stay 1, app stays 1, epoch stays 0
    let res_unavail = confidential_generate_attested_with_readiness(
        &request("req-unavail"),
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
        NvattestEnsureStatus::Unavailable,
    );
    assert!(matches!(
        res_unavail,
        ConfidentialResult::AttestationNotVerified
    ));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 1);
    assert_eq!(server.connection(0).app_requests.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.confidential_channel_pool().epoch(), 0);

    // 3. Invalid URL -> AttestationFailed("tls_handshake_failed"), prefaces stay 1, app stays 1, epoch stays 0
    let mut ep_invalid = endpoint.clone();
    ep_invalid.base_url = "not-a-url".into();
    let res_invalid = confidential_generate_attested(
        &request("req-inv"),
        &journal_path,
        &ep_invalid,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res_invalid), Some("tls_handshake_failed"));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 1);
    assert_eq!(server.connection(0).app_requests.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.confidential_channel_pool().epoch(), 0);

    // 4. Normal call still does not preface
    let res_norm = confidential_generate_attested(
        &request("req-norm"),
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_norm, ConfidentialResult::Generated(_)));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 1);

    // 5. Drain idle pool -> next call prefaces
    runtime.drain_idle_pool();
    let res_drain = confidential_generate_attested(
        &request("req-drain"),
        &journal_path,
        &endpoint,
        &config,
        &runtime,
        &AcceptingCompositeVerifier,
    );
    assert!(matches!(res_drain, ConfidentialResult::Generated(_)));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 2);

    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_11_disabled_resumption_full_handshake() {
    let shared_ticketer = Arc::new(DummyTicketer);
    let verify_count = Arc::new(AtomicUsize::new(0));
    let resolve_count = Arc::new(AtomicUsize::new(0));

    let server = TestServer::spawn_plan(ServerPlan {
        default_script: ConnScript {
            hold_handshake: false,
            proof: ProofScript::Valid,
            app: vec![AppScript::Ok("hello".to_owned())],
            without_evidence: false,
        },
        shared_ticketer: Some(shared_ticketer),
        resolver_count: Some(resolve_count.clone()),
        conn_scripts: Vec::new(),
    });
    let journal_path = temp_journal("oracle_11");
    let endpoint = test_endpoint(server.port);
    let config = Map::new();
    let verifier = CountingCompositeVerifier {
        verify_count: verify_count.clone(),
    };

    let rt1 = EndpointRuntime::default();
    let res1 = confidential_generate_attested(
        &request("req-1"),
        &journal_path,
        &endpoint,
        &config,
        &rt1,
        &verifier,
    );
    assert!(matches!(res1, ConfidentialResult::Generated(_)));

    let rt2 = EndpointRuntime::default();
    let res2 = confidential_generate_attested(
        &request("req-2"),
        &journal_path,
        &endpoint,
        &config,
        &rt2,
        &verifier,
    );
    assert!(matches!(res2, ConfidentialResult::Generated(_)));

    assert_eq!(verify_count.load(Ordering::SeqCst), 2);
    assert_eq!(resolve_count.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(&journal_path);
}

#[test]
fn oracle_12_proof_surplus_fails_establishment() {
    let journal_path = temp_journal("oracle_12");
    let config = Map::new();

    // 1. Surplus proof response
    let server_surplus = TestServer::spawn_plan(ServerPlan {
        default_script: ConnScript {
            hold_handshake: false,
            proof: ProofScript::Surplus,
            app: vec![AppScript::Ok("never-reached".to_owned())],
            without_evidence: false,
        },
        ..Default::default()
    });
    let runtime1 = EndpointRuntime::default();
    let ep1 = test_endpoint(server_surplus.port);

    let res1 = confidential_generate_attested(
        &request("req-surplus"),
        &journal_path,
        &ep1,
        &config,
        &runtime1,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res1), Some("proof_http_failed"));
    assert_eq!(
        server_surplus
            .stats
            .app_requests_read
            .load(Ordering::SeqCst),
        0
    );

    // 2. BareLf proof response
    let server_bare = TestServer::spawn_plan(ServerPlan {
        default_script: ConnScript {
            hold_handshake: false,
            proof: ProofScript::BareLf,
            app: vec![AppScript::Ok("never-reached".to_owned())],
            without_evidence: false,
        },
        ..Default::default()
    });
    let runtime2 = EndpointRuntime::default();
    let ep2 = test_endpoint(server_bare.port);

    let res2 = confidential_generate_attested(
        &request("req-bare"),
        &journal_path,
        &ep2,
        &config,
        &runtime2,
        &AcceptingCompositeVerifier,
    );
    assert_eq!(reason_code(&res2), Some("proof_http_failed"));
    assert_eq!(
        server_bare.stats.app_requests_read.load(Ordering::SeqCst),
        0
    );

    let _ = fs::remove_dir_all(&journal_path);
}

/// Accepts with an offline signed-age status that has `remaining` left at
/// verification time on the pool's clock.
struct OfflineStatusVerifier {
    clock: Arc<MockClock>,
    remaining: Duration,
}

impl CompositeVerifier for OfflineStatusVerifier {
    fn verify(
        &self,
        _: CpuBundle<'_>,
        _: CompositeVerificationInput<'_>,
    ) -> Result<CompositeVerdict, CompositeVerificationError> {
        let now = self.clock.now_system();
        let mut verdict = test_verdict();
        verdict.gpu.status =
            solstone_core_spp_attest::nvgpu::GpuStatusAuthorization::OfflineSignedAge {
                verified_at: now,
                deadline: now + self.remaining,
            };
        Ok(verdict)
    }
}

#[test]
fn oracle_13_offline_status_channels_keep_the_reuse_window_and_short_status_is_refused() {
    let journal_path = temp_journal("oracle_13");
    let config = Map::new();
    let server = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let clock = Arc::new(MockClock::new());
    let runtime = EndpointRuntime::with_pool_clock(1, clock.clone());
    let endpoint = test_endpoint(server.port);
    let verifier = OfflineStatusVerifier {
        clock: clock.clone(),
        remaining: Duration::from_secs(130),
    };
    let generate = |id: &str| {
        confidential_generate_attested(
            &request(id),
            &journal_path,
            &endpoint,
            &config,
            &runtime,
            &verifier,
        )
    };

    // Admitted with exactly 130 s of status left.
    assert!(matches!(
        generate("req-1"),
        ConfidentialResult::Generated(_)
    ));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 1);
    // Reused inside the window. (The pool's 120 s reuse age already covers
    // this; the per-request status check is defence in depth, exercised by
    // the transport unit test.)
    clock.advance_both(Duration::from_secs(60));
    assert!(matches!(
        generate("req-2"),
        ConfidentialResult::Generated(_)
    ));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 1);
    // 121 s after admission it starts no new request: a fresh channel is
    // attested instead.
    clock.advance_both(Duration::from_secs(61));
    assert!(matches!(
        generate("req-3"),
        ConfidentialResult::Generated(_)
    ));
    assert_eq!(server.stats.prefaces_read.load(Ordering::SeqCst), 2);

    // A status with 129 s left is refused at admission and sends nothing.
    let short_server = TestServer::spawn(AppScript::Ok("hello".to_owned()));
    let short_clock = Arc::new(MockClock::new());
    let short_runtime = EndpointRuntime::with_pool_clock(1, short_clock.clone());
    let short = confidential_generate_attested(
        &request("req-short"),
        &journal_path,
        &test_endpoint(short_server.port),
        &config,
        &short_runtime,
        &OfflineStatusVerifier {
            clock: short_clock,
            remaining: Duration::from_secs(129),
        },
    );
    assert_eq!(reason_code(&short), Some("status_deadline_insufficient"));
    assert_eq!(short_server.stats.prefaces_read.load(Ordering::SeqCst), 1);
    assert_eq!(
        short_server.stats.app_requests_read.load(Ordering::SeqCst),
        0
    );

    let _ = fs::remove_dir_all(&journal_path);
}
