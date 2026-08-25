// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-boundary RA-TLS channel tests that bind loopback sockets.

use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, SystemTime},
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
use socket2::{Domain, Protocol, Socket, Type};
use solstone_core_spp_attest::{
    CpuBundle,
    nvgpu::claims::GpuAppraisal,
    snp::{CpuAppraisal, CpuTcb, TcbVersion},
};
use solstone_core_spp_ratls::{
    AttestationFailureKind, AttestationStateStore, AttestedChannel, CompositeVerdict,
    CompositeVerificationError, CompositeVerificationInput, CompositeVerifier, RatlsChannelError,
    RatlsEndpoint, classify_channel_failure, establish_attested_channel,
    ratls::contract::{
        COMPOSITE_EVIDENCE_OID, CompositeEvidence, EXPORTER_BYTES, EXPORTER_LABEL,
        EXPORTER_PROOF_MEDIA_TYPE, ExporterProof, exporter_binding, exporter_context,
    },
    send_json_request,
};
use x509_parser::{pem::parse_x509_pem, prelude::FromDer};

const MAX_PROOF_RESPONSE_HEADERS: usize = 16 * 1024;
const MAX_PROOF_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const THREAD_TIMEOUT: Duration = Duration::from_secs(3);

struct AcceptingCompositeVerifier;
struct RejectingCompositeVerifier;

const TEST_AK_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDd4QO8Gw8LEy9I\nBWtcCN/nqi8GQAwYRMrPKcvR98vqWF6VImZoRQVLotl9ud+k/oSzDH/APeKzdhlH\nyMhHedGJxDProAmegyeIi7TtekICkM811sslns7kyCaCRRV90nQ0Gzmf7WWCsOJX\nRwAH3YQlAJAMfK/SLRnoVb3bG1qwfnCOrNuhsmcMZt7baMH8HpG1/rm39KrRFnow\n5lyIBuoGH9TXuellFb4MTMf8ntXMosqOHe4R3LpfaJ17KjBZkGPPwy+PRiVidfYt\nKkwrCWS30XPfsahxCIF8+aFFmb12fpDvR7g4K+7xrhmfL8SXx00n5C00ucmUSCRT\n0QekpVujAgMBAAECggEADkrHwE6n5ek68vMyaq/BqH0MYWUvwkJwI+8Hy4MgNfyy\nPv4DxbSodipLwy79anXgm13zPrFd0HyLfVXAHOaKakrio0tgQz8khUWmhmOJK/wi\n9M9cr5QutIr1/A8yJrQvOwoD6LrUfpohQkj3BgqtT+rc3IkNlEbGc/JN8/arnVGw\n73DA82n4ajY16mlGBTVVMVmBr77D1as0fAUF8mH5S1mCkmdHz1FJOyEDG3F4C/Gm\nR6ZAnEN/dd4eCTI4GMcfrUZV/LXZuwE2xkUoCMP/7h7S+PA08RIDLZu/b/2c4kZp\nN37Hr+BvmPRpui74NzQszMXvpuLVxTr0/CebRisr5QKBgQD95euOEySX0bsyyQsq\ngwPycxpTJl9h0xUBUuZ69zRXMbYY6tc7xAlQaZSYaNfsFGLvPYxelZGVgXbw+nEI\nKoFuz70AI8HrDX1fb6IOQ6NwGT2qPkqZpySmJC9CQ8WcRwX8sdKVrigHPvI0i4tt\nQh4DOYOgmo1BTnWYRk0EuCEX5wKBgQDftzyvg4WaVp5xyZVyc8bCooMwfbnRKvBk\nYwwpxAJZQssRtWgy6gZ7f1m4wIaxmdNay9oKzMoGEaq6hb23uTe90nNyNtvb69I/\nBdJT6/HA8fmRyyyLqfcBLhpiqVlWWEuk1NHvti96VkjgVz7scVIV34kvEqPV+z/X\nVT/ibvX25QKBgQCaQ3cydIkYQVL3EVXad44PYkYNXVQ4sLKjgkYNUmOX0tlsHEu3\nwW1TUUL6s0D17JEMAR5nXYL+DpJA6jmBF6patJeGHTO2aBTTxpT1C72i34MrC/vx\nja9jzrp0DY9kW3bUyQpE7XLerC0nJd4J/VEU7n3+N8k5c71ZTuV+x4074wKBgCuE\nLR3G65oV90QS/isBMkxx6CrqidaSD6i3S4pkQkCyqWWMb/RXaWNkZkN1z72EOoSS\n2pr3MuTzUs5tbXXrZVhbM3GoEiQ5PvBbZYpFfwUVDIK7jrKsIQvtt9wxLNuK2Uv6\nyctjGOEnH43j6q17bYgrrzek3JGnCcgNIRwekWGxAoGAe6Et5hGK+jLoRmUIvoZw\nBriUxphnVgDIvZpWb3ZYNdKlHHVQh4GwClJ3cfFoTYKjI1KRdUjEAZg8dAmAr+gN\niEljJylwJzlVN6TJ43FQ2UwqcglRHGyir9lyfCaNvDJqAuGSfGF6AhRZl7MSpcou\nE9nC0XWcWIVtpOJj3T+19Jk=\n-----END PRIVATE KEY-----\n";
const TEST_AK_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA3eEDvBsPCxMvSAVrXAjf\n56ovBkAMGETKzynL0ffL6lhelSJmaEUFS6LZfbnfpP6Eswx/wD3is3YZR8jIR3nR\nicQz66AJnoMniIu07XpCApDPNdbLJZ7O5MgmgkUVfdJ0NBs5n+1lgrDiV0cAB92E\nJQCQDHyv0i0Z6FW92xtasH5wjqzbobJnDGbe22jB/B6Rtf65t/Sq0RZ6MOZciAbq\nBh/U17npZRW+DEzH/J7VzKLKjh3uEdy6X2ideyowWZBjz8Mvj0YlYnX2LSpMKwlk\nt9Fz37GocQiBfPmhRZm9dn6Q70e4OCvu8a4Zny/El8dNJ+QtNLnJlEgkU9EHpKVb\nowIDAQAB\n-----END PUBLIC KEY-----\n";

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

#[derive(Debug)]
struct StaticCertResolver(Arc<CertifiedKey>);

impl ResolvesServerCert for StaticCertResolver {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

fn server_config(
    versions: &[&'static rustls::SupportedProtocolVersion],
    certificate: CertificateDer<'static>,
    private_key: PrivateKeyDer<'static>,
) -> ServerConfig {
    let provider = rustls::crypto::ring::default_provider();
    let signing_key = provider
        .key_provider
        .load_private_key(private_key)
        .expect("test signing key");
    let certified_key = CertifiedKey::new(vec![certificate], signing_key);
    ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(versions)
        .expect("test protocol versions")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(StaticCertResolver(Arc::new(certified_key))))
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

    let (_, private_key) = parse_x509_pem(TEST_AK_PRIVATE_PEM.as_bytes()).expect("test AK PEM");
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

fn proof_response(proof: ExporterProof) -> Vec<u8> {
    let proof = proof.to_der();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {EXPORTER_PROOF_MEDIA_TYPE}\r\nContent-Length: {}\r\n\r\n",
        proof.len()
    )
    .into_bytes()
    .into_iter()
    .chain(proof)
    .collect()
}

fn certificate_with_evidence(
    owner_nonce: &[u8],
    critical: bool,
    wrong_spki: bool,
) -> (ServerConfig, CompositeEvidence) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
    let params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
    let base_certificate = params.self_signed(&key).expect("base certificate");
    let (_, parsed) = x509_parser::prelude::X509Certificate::from_der(base_certificate.der())
        .expect("parse base certificate");
    let mut evidence = CompositeEvidence {
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
    if wrong_spki {
        evidence.tls_spki_der[0] ^= 0x01;
    }
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
    extension.set_criticality(critical);
    params.custom_extensions.push(extension);
    let certificate = params.self_signed(&key).expect("evidence certificate");
    let config = server_config(
        &[&rustls::version::TLS13],
        CertificateDer::from(certificate.der().to_vec()),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    );
    (config, evidence)
}

fn certificate_without_evidence() -> ServerConfig {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
    let params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
    let certificate = params.self_signed(&key).expect("certificate");
    server_config(
        &[&rustls::version::TLS13],
        CertificateDer::from(certificate.der().to_vec()),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
}

enum GatewayResponse {
    None,
    Static(Vec<u8>),
    ValidProof(Box<CompositeEvidence>),
}

struct GatewayPlan {
    proof: GatewayResponse,
    application_response: Option<Vec<u8>>,
}

#[derive(Default)]
struct GatewayObservation {
    preface: Vec<u8>,
    exporter_request: Vec<u8>,
    application_request: Vec<u8>,
}

struct Gateway {
    port: u16,
    observed: mpsc::Receiver<GatewayObservation>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Gateway {
    fn finish(mut self) -> GatewayObservation {
        let observation = self.observed.recv_timeout(THREAD_TIMEOUT);
        let joined = join_bounded(&mut self.handle);
        assert!(
            joined.is_ok(),
            "{}",
            joined.expect_err("failed join has detail")
        );
        observation.expect("gateway observation arrives within bound")
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = join_bounded(&mut self.handle);
    }
}

fn join_bounded(handle: &mut Option<thread::JoinHandle<()>>) -> Result<(), String> {
    let Some(handle_ref) = handle.as_ref() else {
        return Ok(());
    };
    let deadline = std::time::Instant::now() + THREAD_TIMEOUT;
    while !handle_ref.is_finished() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if !handle_ref.is_finished() {
        handle.take();
        return Err("gateway thread exceeded join bound".to_owned());
    }
    handle
        .take()
        .expect("gateway handle remains owned")
        .join()
        .map_err(|_| "gateway thread panicked".to_owned())
}

fn read_http_request(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() == MAX_PROOF_RESPONSE_HEADERS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers exceed fixture cap",
            ));
        }
        reader.read_exact(&mut byte)?;
        bytes.push(byte[0]);
    }
    let header = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request header UTF-8"))?;
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if content_length > 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body exceeds fixture cap",
        ));
    }
    let header_len = bytes.len();
    bytes.resize(header_len + content_length, 0);
    reader.read_exact(&mut bytes[header_len..])?;
    Ok(bytes)
}

fn start_gateway(config: ServerConfig, plan: GatewayPlan) -> Gateway {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    listener.set_nonblocking(true).expect("nonblocking accept");
    let (observed_sender, observed_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let accept_deadline = std::time::Instant::now() + IO_TIMEOUT;
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < accept_deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("bounded gateway accept failed: {error}"),
            }
        };
        socket
            .set_nonblocking(false)
            .expect("make accepted gateway connection blocking");
        socket
            .set_read_timeout(Some(IO_TIMEOUT))
            .expect("read timeout");
        socket
            .set_write_timeout(Some(IO_TIMEOUT))
            .expect("write timeout");
        let mut observation = GatewayObservation::default();
        observation.preface.resize(b"SPPRAT1\0".len() + 32, 0);
        if socket.read_exact(&mut observation.preface).is_err() {
            observed_sender.send(observation).expect("observation send");
            return;
        }
        let mut stream = StreamOwned::new(
            ServerConnection::new(Arc::new(config)).expect("server connection"),
            socket,
        );
        while stream.conn.is_handshaking() {
            if stream.conn.complete_io(&mut stream.sock).is_err() {
                observed_sender.send(observation).expect("observation send");
                return;
            }
        }
        observation.exporter_request = read_http_request(&mut stream).unwrap_or_default();
        let response = match plan.proof {
            GatewayResponse::None => None,
            GatewayResponse::Static(response) => Some(response),
            GatewayResponse::ValidProof(evidence) => {
                let mut exporter = [0; EXPORTER_BYTES];
                stream
                    .conn
                    .export_keying_material(
                        &mut exporter,
                        EXPORTER_LABEL,
                        Some(&exporter_context(
                            &evidence.owner_nonce,
                            &evidence.tls_spki_der,
                        )),
                    )
                    .expect("server exporter");
                let binding = exporter_binding(
                    &evidence.owner_nonce,
                    &evidence.tls_spki_der,
                    &exporter,
                    &evidence.gpu_envelope,
                );
                let (quote_message, quote_signature, quote_pcrs) = test_quote(&binding);
                Some(proof_response(ExporterProof {
                    owner_nonce: evidence.owner_nonce,
                    tls_spki_der: evidence.tls_spki_der,
                    tls_exporter: exporter.to_vec(),
                    quote_message,
                    quote_signature,
                    quote_pcrs,
                }))
            }
        };
        if let Some(response) = response {
            stream.write_all(&response).expect("proof response");
        }
        if let Some(response) = plan.application_response {
            observation.application_request =
                read_http_request(&mut stream).expect("bounded application request");
            stream.write_all(&response).expect("application response");
        }
        observed_sender.send(observation).expect("observation send");
    });
    Gateway {
        port,
        observed: observed_receiver,
        handle: Some(handle),
    }
}

fn endpoint(port: u16) -> RatlsEndpoint {
    RatlsEndpoint::new("127.0.0.1", port)
}

fn establish(
    port: u16,
    verifier: &dyn CompositeVerifier,
) -> Result<AttestedChannel, RatlsChannelError> {
    establish_attested_channel(
        &endpoint(port),
        &[7; 32],
        Path::new("."),
        SystemTime::UNIX_EPOCH,
        None,
        None,
        None,
        verifier,
        Duration::from_secs(2),
        4,
    )
}

fn exact_preface() -> Vec<u8> {
    b"SPPRAT1\0".iter().copied().chain([7_u8; 32]).collect()
}

fn exact_exporter_request() -> &'static [u8] {
    b"GET /._sol/spp/exporter-proof HTTP/1.1\r\nHost: spp-engine\r\nContent-Length: 0\r\n\r\n"
}

fn assert_preface(observation: &GatewayObservation) {
    assert_eq!(observation.preface, exact_preface());
}

fn static_response(status: u16, body: &[u8]) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes()
    .into_iter()
    .chain(body.iter().copied())
    .collect()
}

fn rejected(result: Result<AttestedChannel, RatlsChannelError>) -> RatlsChannelError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("test gateway unexpectedly established a channel"),
    }
}

#[test]
fn tls_client_rejects_a_tls12_only_gateway() {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
    let certificate = CertificateParams::new(vec!["spp-engine".to_owned()])
        .expect("params")
        .self_signed(&key)
        .expect("cert");
    let config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate.der().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .expect("TLS 1.2 server config");
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::None,
            application_response: None,
        },
    );
    assert_eq!(
        rejected(establish(gateway.port, &AcceptingCompositeVerifier)).reason_code,
        "gateway_unreachable"
    );
    let observation = gateway.finish();
    assert_preface(&observation);
    assert!(observation.exporter_request.is_empty());
}

#[test]
fn certificate_rejection_writes_no_exporter_http_payload_and_closes() {
    let (config, _) = certificate_with_evidence(&[7; 32], true, false);
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::None,
            application_response: None,
        },
    );
    let error = rejected(establish(gateway.port, &RejectingCompositeVerifier));
    assert_eq!(error.reason_code, "composite_appraisal_failed");
    let observation = gateway.finish();
    assert_preface(&observation);
    assert!(observation.exporter_request.is_empty());
}

#[test]
fn exporter_mismatch_rejects_after_a_verified_certificate() {
    let (config, evidence) = certificate_with_evidence(&[7; 32], true, false);
    assert_eq!(
        COMPOSITE_EVIDENCE_OID,
        "2.25.3708997813.3535365757.2172800616.1077671698"
    );
    let proof = ExporterProof {
        owner_nonce: vec![7; 32],
        tls_spki_der: evidence.tls_spki_der.clone(),
        tls_exporter: vec![0; EXPORTER_BYTES],
        quote_message: Vec::new(),
        quote_signature: Vec::new(),
        quote_pcrs: Vec::new(),
    }
    .to_der();
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {EXPORTER_PROOF_MEDIA_TYPE}\r\nContent-Length: {}\r\n\r\n", proof.len()).into_bytes().into_iter().chain(proof).collect();
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::Static(response),
            application_response: None,
        },
    );
    assert_eq!(
        rejected(establish(gateway.port, &AcceptingCompositeVerifier)).reason_code,
        "exporter_mismatch"
    );
    let observation = gateway.finish();
    assert_preface(&observation);
    assert_eq!(observation.exporter_request, exact_exporter_request());
}

#[test]
fn proof_headers_cannot_overshoot_the_cap() {
    let (config, _) = certificate_with_evidence(&[7; 32], true, false);
    let prefix = b"HTTP/1.1 200 OK\r\nX: ";
    let suffix = b"\r\nContent-Length: 0\r\n\r\n";
    let padding_len = MAX_PROOF_RESPONSE_HEADERS + 1 - prefix.len() - (suffix.len() - 4);
    let response = prefix
        .iter()
        .copied()
        .chain(std::iter::repeat_n(b'a', padding_len))
        .chain(suffix.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(
        response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("response marker"),
        MAX_PROOF_RESPONSE_HEADERS + 1
    );
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::Static(response),
            application_response: None,
        },
    );
    assert_eq!(
        rejected(establish(gateway.port, &AcceptingCompositeVerifier)).reason_code,
        "proof_http_failed"
    );
    let observation = gateway.finish();
    assert_eq!(observation.exporter_request, exact_exporter_request());
}

#[test]
fn proof_body_length_cannot_exceed_the_cap() {
    let (config, _) = certificate_with_evidence(&[7; 32], true, false);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        MAX_PROOF_RESPONSE_BYTES + 1
    )
    .into_bytes();
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::Static(response),
            application_response: None,
        },
    );
    assert_eq!(
        rejected(establish(gateway.port, &AcceptingCompositeVerifier)).reason_code,
        "proof_http_failed"
    );
    let observation = gateway.finish();
    assert_eq!(observation.exporter_request, exact_exporter_request());
}

#[test]
fn proof_headers_require_crlf_framing() {
    let (config, _) = certificate_with_evidence(&[7; 32], true, false);
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::Static(b"HTTP/1.1 200 OK\nContent-Length: 0\r\n\r\n".to_vec()),
            application_response: None,
        },
    );
    assert_eq!(
        rejected(establish(gateway.port, &AcceptingCompositeVerifier)).reason_code,
        "proof_http_failed"
    );
    let observation = gateway.finish();
    assert_eq!(observation.exporter_request, exact_exporter_request());
}

#[test]
fn non_200_exporter_proof_is_rejected() {
    let (config, _) = certificate_with_evidence(&[7; 32], true, false);
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::Static(static_response(503, b"unavailable")),
            application_response: None,
        },
    );
    assert_eq!(
        rejected(establish(gateway.port, &AcceptingCompositeVerifier)).reason_code,
        "proof_http_failed"
    );
    let observation = gateway.finish();
    assert_eq!(observation.exporter_request, exact_exporter_request());
}

#[test]
fn exact_spprat1_preface_nonce_and_exporter_request_establish_channel() {
    let (config, evidence) = certificate_with_evidence(&[7; 32], true, false);
    let gateway = start_gateway(
        config,
        GatewayPlan {
            proof: GatewayResponse::ValidProof(Box::new(evidence)),
            application_response: None,
        },
    );
    let channel = establish(gateway.port, &AcceptingCompositeVerifier).expect("attested channel");
    assert_eq!(channel.verified.verdict, test_verdict());
    let observation = gateway.finish();
    assert_preface(&observation);
    assert_eq!(observation.exporter_request, exact_exporter_request());
}

#[test]
fn held_bound_not_listening_socket_is_unreachable_without_port_race() {
    let socket =
        Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).expect("refusal socket");
    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())
        .expect("bind refusal socket without listen");
    socket
        .set_nonblocking(true)
        .expect("refusal socket can be inspected without blocking");
    let accept_error = socket
        .accept()
        .expect_err("bound-only refusal fixture must not be listening");
    assert_ne!(
        accept_error.kind(),
        io::ErrorKind::WouldBlock,
        "a listening nonblocking fixture would wait for a connection"
    );
    let port = socket
        .local_addr()
        .expect("refusal address")
        .as_socket()
        .expect("IP address")
        .port();
    assert_eq!(
        rejected(establish(port, &AcceptingCompositeVerifier)).reason_code,
        "gateway_unreachable"
    );
}

#[test]
fn certificate_evidence_nonce_spki_criticality_and_presence_are_enforced() {
    for (config, expected) in [
        (
            certificate_with_evidence(&[8; 32], true, false).0,
            "nonce_mismatch",
        ),
        (
            certificate_with_evidence(&[7; 32], true, true).0,
            "spki_mismatch",
        ),
        (
            certificate_with_evidence(&[7; 32], false, false).0,
            "certificate_extension_not_critical",
        ),
        (
            certificate_without_evidence(),
            "certificate_extension_missing",
        ),
    ] {
        let gateway = start_gateway(
            config,
            GatewayPlan {
                proof: GatewayResponse::None,
                application_response: None,
            },
        );
        let error = rejected(establish(gateway.port, &AcceptingCompositeVerifier));
        assert_eq!(error.reason_code, expected);
        let observation = gateway.finish();
        assert_preface(&observation);
        assert!(observation.exporter_request.is_empty());
    }
}

#[test]
fn live_post_attestation_json_request_has_exact_framing_with_and_without_authorization() {
    let body = br#"{"model":"fixture","messages":[]}"#;
    for credential in [None, Some("fixture-token")] {
        let (config, evidence) = certificate_with_evidence(&[7; 32], true, false);
        let response_body = br#"{"ok":true}"#;
        let gateway = start_gateway(
            config,
            GatewayPlan {
                proof: GatewayResponse::ValidProof(Box::new(evidence)),
                application_response: Some(static_response(200, response_body)),
            },
        );
        let mut channel =
            establish(gateway.port, &AcceptingCompositeVerifier).expect("attested channel");
        let response = send_json_request(
            &mut channel,
            "spp-engine",
            "/v1/chat/completions",
            credential,
            body,
        )
        .expect("application response");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, response_body);
        let observation = gateway.finish();
        assert_preface(&observation);
        assert_eq!(observation.exporter_request, exact_exporter_request());
        let authorization = credential
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        let expected = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: spp-engine\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{authorization}\r\n{}",
            body.len(),
            std::str::from_utf8(body).expect("fixture JSON")
        );
        assert_eq!(observation.application_request, expected.as_bytes());
    }
}

#[test]
fn missing_evidence_teardown_precedes_failure_recording() {
    let gateway = start_gateway(
        certificate_without_evidence(),
        GatewayPlan {
            proof: GatewayResponse::None,
            application_response: None,
        },
    );
    let error = rejected(establish(gateway.port, &AcceptingCompositeVerifier));
    assert_ne!(error.reason_code, "gateway_unreachable");
    assert_eq!(
        classify_channel_failure(error.reason_code),
        AttestationFailureKind::Failed
    );
    let observation = gateway.finish();
    assert_preface(&observation);
    assert!(observation.exporter_request.is_empty());
    let store = AttestationStateStore::new();
    store.record_attestation_failed(
        classify_channel_failure(error.reason_code),
        error.reason_code,
    );
    let state = store.get_attestation_state();
    assert!(state.session.is_none());
    assert_eq!(
        state.failure.expect("failure").reason_code,
        "certificate_extension_missing"
    );
}
