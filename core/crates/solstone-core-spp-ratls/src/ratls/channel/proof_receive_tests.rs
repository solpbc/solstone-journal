// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::proof_receive::ReceiveStage;
use std::collections::VecDeque;
use std::io::Cursor;

#[derive(Clone)]
enum Input {
    Bytes(Vec<u8>),
    Error(ErrorKind),
}
#[derive(Clone)]
struct Script {
    input: VecDeque<Input>,
    reads: Vec<(usize, usize)>,
    nonblocking: Vec<bool>,
    fail_nonblocking: Option<bool>,
}
impl Script {
    fn new(input: Vec<Input>) -> Self {
        Self {
            input: input.into(),
            reads: Vec::new(),
            nonblocking: Vec::new(),
            fail_nonblocking: None,
        }
    }
}
impl Read for Script {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let result = match self.input.pop_front() {
            Some(Input::Bytes(mut bytes)) => {
                let n = buffer.len().min(bytes.len());
                buffer[..n].copy_from_slice(&bytes[..n]);
                if n < bytes.len() {
                    bytes.drain(..n);
                    self.input.push_front(Input::Bytes(bytes));
                }
                Ok(n)
            }
            Some(Input::Error(kind)) => Err(std::io::Error::from(kind)),
            None => Ok(0),
        };
        self.reads
            .push((buffer.len(), result.as_ref().copied().unwrap_or(usize::MAX)));
        result
    }
}
fn bytes(value: &[u8]) -> Input {
    Input::Bytes(value.to_vec())
}
fn peer(n: u8) -> SocketAddr {
    SocketAddr::from(([10, 0, 0, n], 443))
}
const HEADER: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n";

#[test]
fn selected_failure_stays_with_strict_rank_and_attempt_count() {
    for (reasons, chosen) in [
        (
            [
                "proof_http_eof",
                "proof_http_timeout",
                "gateway_unreachable",
            ],
            0,
        ),
        (
            ["gateway_unreachable", "proof_http_eof", "proof_http_eof"],
            1,
        ),
        (["proof_http_eof", "pcr_pin_mismatch", "proof_http_eof"], 1),
    ] {
        let mut i = 0;
        let error = establish_over_addresses::<()>(
            &[peer(1), peer(2), peer(3)],
            &[0; 32],
            |n| Ok(vec![0; n]),
            || true,
            |address, _| {
                let reason = reasons[i];
                i += 1;
                observe_attempt(address, |_| Err(RatlsChannelError::new(reason)))
            },
        )
        .unwrap_err();
        assert_eq!(error.reason_code, reasons[chosen]);
        let summary = error.selected_failure.unwrap();
        assert_eq!(summary.destination, peer((chosen + 1) as u8));
        assert_eq!(summary.ordinal, (chosen + 1) as u8);
        assert_eq!(summary.attempted, 3);
    }
    let mut calls = 0;
    let success = establish_over_addresses(
        &[peer(1), peer(2)],
        &[0; 32],
        |n| Ok(vec![0; n]),
        || true,
        |address, _| {
            calls += 1;
            if calls == 1 {
                observe_attempt(address, |_| Err(RatlsChannelError::new("proof_http_eof")))
            } else {
                Ok(7)
            }
        },
    );
    assert_eq!(success, Ok(7));
    let budget = establish_over_addresses::<()>(
        &[peer(1), peer(2)],
        &[0; 32],
        |n| Ok(vec![0; n]),
        || false,
        |address, _| observe_attempt(address, |_| Err(RatlsChannelError::new("proof_http_eof"))),
    )
    .unwrap_err();
    assert_eq!(budget.selected_failure.unwrap().attempted, 1);
    let nonce = establish_over_addresses::<()>(
        &[peer(1), peer(2)],
        &[0; 32],
        |_| Err(RatlsChannelError::new("nonce_generation_failed")),
        || true,
        |address, _| observe_attempt(address, |_| Err(RatlsChannelError::new("proof_http_eof"))),
    )
    .unwrap_err();
    assert!(nonce.selected_failure.is_none());
    let local = observe_attempt::<()>(peer(2), |_| {
        Err(RatlsChannelError::new("nvattest_unavailable"))
    })
    .unwrap_err();
    assert!(local.selected_failure.is_none());
}

#[test]
fn production_attachment_retains_actual_parser_progress_and_post_receive_refusal() {
    for input in [Vec::new(), b"HTT".to_vec(), [HEADER, b"a"].concat()] {
        let error = observe_attempt(peer(1), |summary| {
            recv_proof_body_observed(&mut Cursor::new(&input), Some(&mut summary.progress))
        })
        .unwrap_err();
        assert_eq!(error.reason_code, "proof_http_eof");
        let p = error.selected_failure.unwrap().progress;
        if input.len() < HEADER.len() {
            assert_eq!(p.stage, ReceiveStage::Headers);
            assert_eq!(p.category, ReceiveCategory::ResponseEof);
            assert_eq!(p.header_bytes, Some(input.len()));
            assert_eq!(p.body_bytes, None);
            assert_eq!(p.declared_body_bytes, None);
        } else {
            assert_eq!(p.stage, ReceiveStage::Body);
            assert_eq!(p.category, ReceiveCategory::ResponseBodyEof);
            assert_eq!(p.header_bytes, Some(HEADER.len()));
            assert_eq!(p.body_bytes, Some(1));
            assert_eq!(p.declared_body_bytes, Some(3));
        }
    }
    let error = observe_attempt::<()>(peer(3), |summary| {
        recv_proof_body_observed(
            &mut Cursor::new([HEADER, b"abc"].concat()),
            Some(&mut summary.progress),
        )?;
        Err(RatlsChannelError::new("pcr_pin_mismatch"))
    })
    .unwrap_err();
    assert_eq!(error.reason_code, "pcr_pin_mismatch");
    let p = error.selected_failure.unwrap().progress;
    assert_eq!(p.stage, ReceiveStage::Complete);
    assert_eq!(p.category, ReceiveCategory::ProofRejected);
    assert_eq!(
        (p.header_bytes, p.body_bytes, p.declared_body_bytes),
        (Some(HEADER.len()), Some(3), Some(3))
    );
}

#[test]
fn actual_proof_parser_observation_preserves_results_and_io() {
    let mut cases = vec![
        vec![],
        vec![bytes(b"HTT")],
        vec![bytes(&[HEADER, b"a"].concat())],
        vec![
            Input::Error(ErrorKind::Interrupted),
            bytes(&[HEADER, b"abc"].concat()),
        ],
        vec![bytes(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        )],
        vec![bytes(b"HTTP/1.1 500 No\r\nContent-Length: 0\r\n\r\n")],
        vec![bytes(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Type: text/plain\r\n\r\n",
        )],
        vec![bytes(&[HEADER, b"abcd"].concat())],
    ];
    cases.push(
        [HEADER, b"abc"]
            .concat()
            .into_iter()
            .map(|b| bytes(&[b]))
            .collect(),
    );
    for kind in [
        ErrorKind::UnexpectedEof,
        ErrorKind::TimedOut,
        ErrorKind::WouldBlock,
        ErrorKind::ConnectionReset,
        ErrorKind::Other,
    ] {
        cases.push(vec![bytes(b"HTT"), Input::Error(kind)]);
        cases.push(vec![bytes(&[HEADER, b"a"].concat()), Input::Error(kind)]);
    }
    for input in cases {
        let mut plain = Script::new(input.clone());
        let expected = recv_proof_body(&mut plain);
        for enabled in [false, true] {
            let mut observed = Script::new(input.clone());
            let mut progress = ReceiveProgress::default();
            let actual = recv_proof_body_observed(&mut observed, enabled.then_some(&mut progress));
            assert_eq!(actual, expected);
            assert_eq!(observed.reads, plain.reads);
        }
    }
    for (kind, category) in [
        (ErrorKind::UnexpectedEof, ReceiveCategory::IoUnexpectedEof),
        (ErrorKind::TimedOut, ReceiveCategory::Timeout),
        (ErrorKind::ConnectionReset, ReceiveCategory::Reset),
    ] {
        let mut p = ReceiveProgress::default();
        let mut stream = Script::new(vec![bytes(&[HEADER, b"a"].concat()), Input::Error(kind)]);
        assert!(recv_proof_body_observed(&mut stream, Some(&mut p)).is_err());
        assert_eq!(
            (
                p.stage,
                p.category,
                p.header_bytes,
                p.body_bytes,
                p.declared_body_bytes
            ),
            (
                ReceiveStage::Body,
                category,
                Some(HEADER.len()),
                Some(1),
                Some(3)
            )
        );
    }
}

fn memory_tls() -> (ClientConnection, rustls::ServerConnection) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
    )
    .unwrap();
    let mut server = rustls::ServerConnection::new(Arc::new(config)).unwrap();
    let mut client =
        ClientConnection::new(tls_config().0, ServerName::try_from("localhost").unwrap()).unwrap();
    for _ in 0..10 {
        let mut wire = Vec::new();
        client.write_tls(&mut wire).unwrap();
        server.read_tls(&mut Cursor::new(wire)).unwrap();
        server.process_new_packets().unwrap();
        let mut wire = Vec::new();
        server.write_tls(&mut wire).unwrap();
        client.read_tls(&mut Cursor::new(wire)).unwrap();
        client.process_new_packets().unwrap();
        if !client.is_handshaking() && !server.is_handshaking() {
            return (client, server);
        }
    }
    panic!("memory TLS handshake did not finish");
}

#[test]
fn actual_tls_read_branches_keep_positive_close_separate_from_unknown() {
    for mode in [
        "tcp_eof",
        "close_notify",
        "reset",
        "tls_error",
        "buffered_tls_error",
    ] {
        let mut outcomes = Vec::new();
        for enabled in [false, true] {
            let (mut conn, mut server) = memory_tls();
            let mut wire = Vec::new();
            if mode == "close_notify" {
                server.send_close_notify();
                server.write_tls(&mut wire).unwrap();
            }
            if mode == "buffered_tls_error" {
                server.writer().write_all(b"abc").unwrap();
                server.write_tls(&mut wire).unwrap();
            }
            if mode.ends_with("tls_error") {
                wire.extend_from_slice(&[23, 3, 3, 0, 1, 0]);
            }
            let mut input = if wire.is_empty() {
                vec![]
            } else {
                vec![bytes(&wire)]
            };
            if mode == "reset" {
                input.push(Input::Error(ErrorKind::ConnectionReset));
            }
            let mut stream = Script::new(input);
            let mut eof = false;
            let mut failed = false;
            let mut p = ReceiveTermination::default();
            let mut buffer = [0; 10];
            let result = read_attested(
                &mut conn,
                &mut stream,
                &mut eof,
                &mut failed,
                enabled.then_some(&mut p),
                &mut buffer,
            );
            let outcome = result
                .as_ref()
                .map(|n| buffer[..*n].to_vec())
                .map_err(|e| e.kind());
            outcomes.push((outcome, stream.reads.clone(), eof, failed));
            if enabled {
                assert_eq!(p.saw_eof, eof);
                assert_eq!(p.tls_failed, failed);
                assert_eq!(p.close_observed, mode == "close_notify");
                match mode {
                    "tcp_eof" => {
                        assert!(eof);
                        assert_eq!(p.branch, ReceiveBranch::MappedEof);
                    }
                    "close_notify" => assert_eq!(p.branch, ReceiveBranch::PlaintextZero),
                    "reset" => assert_eq!(p.branch, ReceiveBranch::Io),
                    "tls_error" => assert_eq!(p.branch, ReceiveBranch::TlsRejected),
                    "buffered_tls_error" => {
                        assert_eq!(p.branch, ReceiveBranch::BufferedBeforeTlsFailure);
                        assert_eq!(buffer[..3], *b"abc");
                    }
                    _ => unreachable!(),
                }
            }
        }
        assert_eq!(outcomes[0], outcomes[1], "{mode}");
    }
}

impl Write for Script {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        panic!("unexpected write")
    }
    fn flush(&mut self) -> std::io::Result<()> {
        panic!("unexpected flush")
    }
}
impl AttestedIo for Script {
    fn set_io_deadline(&mut self, _: Option<Instant>) -> std::io::Result<()> {
        panic!("unexpected deadline change")
    }
    fn set_io_timeout(&mut self, _: Option<Duration>) -> std::io::Result<()> {
        panic!("unexpected timeout change")
    }
    fn trailing_after_body(&mut self) -> std::io::Result<Trailing> {
        panic!("unexpected delegated trailing")
    }
}
impl TrailingIo for Script {
    fn set_probe_nonblocking(&mut self, enabled: bool) -> std::io::Result<()> {
        self.nonblocking.push(enabled);
        if self.fail_nonblocking == Some(enabled) {
            return Err(std::io::Error::from(ErrorKind::Other));
        }
        Ok(())
    }
}
#[test]
fn actual_trailing_probe_preserves_io_and_positive_close_state() {
    for mode in [
        "idle",
        "tcp_eof",
        "buffered_plaintext",
        "close_notify",
        "tls_error",
        "wire_bytes",
        "reset",
        "timeout",
        "other_io",
        "setup_fail",
        "restore_fail",
    ] {
        let mut outcomes = Vec::new();
        for enabled in [false, true] {
            let (mut conn, mut server) = memory_tls();
            let mut wire = Vec::new();
            if matches!(mode, "buffered_plaintext" | "wire_bytes") {
                server.writer().write_all(b"x").unwrap();
                server.write_tls(&mut wire).unwrap();
            }
            if mode == "close_notify" {
                server.send_close_notify();
                server.write_tls(&mut wire).unwrap();
            }
            if mode == "tls_error" {
                wire.extend_from_slice(&[23, 3, 3, 0, 1, 0]);
            }
            if !wire.is_empty() && mode != "wire_bytes" {
                conn.read_tls(&mut Cursor::new(&wire)).unwrap();
            }
            let input = match mode {
                "idle" => vec![Input::Error(ErrorKind::WouldBlock)],
                "reset" => vec![Input::Error(ErrorKind::ConnectionReset)],
                "timeout" => vec![Input::Error(ErrorKind::TimedOut)],
                "other_io" => vec![Input::Error(ErrorKind::Other)],
                "wire_bytes" => vec![bytes(&wire)],
                _ => vec![],
            };
            let mut stream = Script::new(input);
            stream.fail_nonblocking = match mode {
                "setup_fail" => Some(true),
                "restore_fail" => Some(false),
                _ => None,
            };
            let mut eof = false;
            let mut failed = false;
            let mut p = ReceiveTermination::default();
            let result = trailing_attested(
                &mut conn,
                &mut stream,
                &mut RecordFramingTracker::new(),
                None,
                &mut eof,
                &mut failed,
                enabled.then_some(&mut p),
            )
            .map_err(|e| e.kind());
            outcomes.push((
                result,
                stream.reads.clone(),
                stream.nonblocking.clone(),
                eof,
                failed,
            ));
            if enabled {
                assert_eq!(p.close_observed, mode == "close_notify");
                assert_eq!(p.saw_eof, eof);
                assert_eq!(p.tls_failed, failed);
                let (expected, branch) = match mode {
                    "idle" => (Ok(Trailing::None), ReceiveBranch::TrailingWouldBlock),
                    "tcp_eof" => (Ok(Trailing::Eof), ReceiveBranch::TrailingTcpEof),
                    "buffered_plaintext" => {
                        (Ok(Trailing::Surplus), ReceiveBranch::TrailingBuffered)
                    }
                    "close_notify" => (Ok(Trailing::Eof), ReceiveBranch::TrailingClose),
                    "tls_error" => (Err(ErrorKind::InvalidData), ReceiveBranch::TlsRejected),
                    "wire_bytes" => (Ok(Trailing::Surplus), ReceiveBranch::TrailingBytes),
                    "reset" => (Ok(Trailing::Surplus), ReceiveBranch::TrailingReset),
                    "timeout" => (Ok(Trailing::Surplus), ReceiveBranch::TrailingTimeout),
                    "other_io" => (Ok(Trailing::Surplus), ReceiveBranch::TrailingIoOther),
                    "setup_fail" => (Ok(Trailing::Surplus), ReceiveBranch::TrailingSetupFailed),
                    "restore_fail" => (Ok(Trailing::Surplus), ReceiveBranch::TrailingRestoreFailed),
                    _ => unreachable!(),
                };
                assert_eq!(result, expected);
                assert_eq!(p.branch, branch, "{mode}");
                if mode == "close_notify" {
                    assert!(stream.reads.is_empty());
                }
                if mode == "restore_fail" {
                    assert!(!eof);
                }
            }
        }
        assert_eq!(outcomes[0], outcomes[1], "{mode}");
    }
}

#[test]
fn private_failure_is_absent_from_ordinary_error_formatting() {
    let error = observe_attempt::<()>(peer(4), |_| Err(RatlsChannelError::new("proof_http_eof")))
        .unwrap_err();
    assert!(error.selected_failure.is_some());
    assert_eq!(
        format!("{error:?}"),
        "RatlsChannelError { reason_code: \"proof_http_eof\" }"
    );
    assert_eq!(
        error.to_string(),
        "confidential attestation rejected (proof_http_eof)"
    );
}
