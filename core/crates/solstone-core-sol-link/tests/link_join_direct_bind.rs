// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `solstone link join` over real TLS against a peer on a direct pair link.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, IsCa, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use solstone_core_sol_client::command::CommandOutput;
use solstone_core_sol_client::seam::{
    LinkJoinCredential, LinkJoinDirectRequest, LinkJoinPairingError, LinkJoinPairingSeam,
    LinkJoinRelayRequest, LinkJoinTerminal, ScriptedHttpTransport, ScriptedLinkJoinPairingSeam,
};
use solstone_core_sol_client_cli::{LinkDispatch, LinkDispatchSeams, dispatch_sol_link_with_seams};
use solstone_core_sol_link::SplLinkJoinPairingSeam;
use spl_core::frame::{FLAG_CLOSE, FLAG_DATA, Frame, FrameDecoder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

struct Journal {
    cert: rcgen::Certificate,
    key: KeyPair,
    jid: String,
}

fn journal() -> Journal {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("CA key");
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    let cert = params.self_signed(&key).expect("CA");
    let spki = spl_core::ca::extract_spki_der(cert.der()).expect("CA SPKI");
    let jid = spl_core::relay_window::jid_from_spki(&spki).expect("CA JID");
    Journal { cert, key, jid }
}

fn words_of(jid: &str) -> (String, String) {
    let spec = spl_core::mark::mark_from_jid(jid)
        .expect("mark")
        .to_render_spec();
    (spec.words[0].to_string(), spec.words[1].to_string())
}

struct Door {
    link: String,
    requests: Arc<Mutex<Vec<String>>>,
    runtime: tokio::runtime::Runtime,
}

/// A door holding `own`'s CA, whose pair link pins that CA, answering the pair
/// request with `claimed_id`.
fn open_door(own: Journal, claimed_id: &str) -> Door {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("leaf key");
    let leaf = CertificateParams::new(vec!["spl.local".to_string()])
        .expect("leaf params")
        .signed_by(&leaf_key, &own.cert, &own.key)
        .expect("leaf");
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .with_no_client_auth()
            .with_single_cert(
                vec![
                    CertificateDer::from(leaf.der().to_vec()),
                    CertificateDer::from(own.cert.der().to_vec()),
                ],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
            )
            .expect("TLS config");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = runtime
        .block_on(TcpListener::bind(("127.0.0.1", 0)))
        .expect("bind");
    let port = listener.local_addr().expect("port").port();

    let mut blob = vec![0x04, 0x01, 127, 0, 0, 1];
    blob.extend_from_slice(&port.to_be_bytes());
    blob.extend_from_slice(&[0x11; 16]);
    blob.extend_from_slice(&spl_core::ca::sha256(own.cert.der())[..16]);
    let link = format!(
        "https://go.solstone.app/p#{}",
        spl_core::crockford::encode(&blob)
    );

    let own = Arc::new(own);
    let claimed_id = claimed_id.to_owned();
    let seen = Arc::clone(&requests);
    runtime.spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let (acceptor, own, claimed_id, seen) = (
                acceptor.clone(),
                Arc::clone(&own),
                claimed_id.clone(),
                Arc::clone(&seen),
            );
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut decoder = FrameDecoder::new();
                let mut request = Vec::new();
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let Ok(count) = tls.read(&mut buf).await else {
                        return;
                    };
                    if count == 0 {
                        return;
                    }
                    decoder.feed(&buf[..count]);
                    for frame in decoder.drain().expect("frames") {
                        if let Some(pong) = frame.control_pong() {
                            let _ = tls.write_all(&pong.encode().expect("pong")).await;
                            continue;
                        }
                        if frame.flags & FLAG_DATA != 0 {
                            request.extend_from_slice(&frame.payload);
                        }
                        if frame.flags & FLAG_CLOSE != 0 {
                            let response = answer(&request, &own, &claimed_id, &seen, port);
                            let reply =
                                Frame::new(frame.stream_id, FLAG_DATA | FLAG_CLOSE, response);
                            let _ = tls.write_all(&reply.encode().expect("reply")).await;
                            let _ = tls.flush().await;
                            let _ = tls.shutdown().await;
                            return;
                        }
                    }
                }
            });
        }
    });
    Door {
        link,
        requests,
        runtime,
    }
}

fn answer(
    request: &[u8],
    own: &Journal,
    claimed_id: &str,
    seen: &Mutex<Vec<String>>,
    port: u16,
) -> Vec<u8> {
    let text = String::from_utf8_lossy(request);
    let (head, body) = text.split_once("\r\n\r\n").expect("request head");
    let start = head.lines().next().expect("request line").to_owned();
    seen.lock().expect("requests").push(start.clone());
    if !start.starts_with("POST ") {
        return b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec();
    }
    let pair: spl_core::PairRequest = serde_json::from_str(body).expect("pair request");
    let client_cert = CertificateSigningRequestParams::from_pem(&pair.csr)
        .expect("CSR")
        .signed_by(&own.cert, &own.key)
        .expect("client cert");
    let (word1, word2) = words_of(claimed_id);
    let payload = serde_json::json!({
        "client_cert": client_cert.pem(),
        "ca_chain": [own.cert.pem()],
        "instance_id": claimed_id,
        "home_label": format!("{word1}\u{b7}{word2}"),
        "fingerprint": format!("sha256:{}", spl_core::ca::sha256_hex(client_cert.der())),
        "home_attestation": "header.payload.signature",
        "local_endpoints": [{"ip": "127.0.0.1", "port": port, "scope": "lan"}],
    })
    .to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    )
    .into_bytes()
}

fn join(config: &Path, link: &str, mark: &str) -> CommandOutput {
    let env = BTreeMap::from([
        ("XDG_CONFIG_HOME".to_string(), config.display().to_string()),
        ("HOME".to_string(), config.display().to_string()),
    ]);
    let args = [
        "link", "join", "--code", link, "--label", "laptop", "--mark", mark,
    ]
    .map(str::to_owned);
    let transport = ScriptedHttpTransport::new(vec![]);
    let seams = LinkDispatchSeams {
        transport: &transport,
        clock: None,
        files: None,
        link_pairing: Some(&SplLinkJoinPairingSeam),
        link_serve: None,
        link_status_probe: None,
    };
    match dispatch_sol_link_with_seams(&args, &env, "", seams) {
        LinkDispatch::Buffered(output) => output,
        LinkDispatch::Resident { .. } => panic!("link join must stay buffered"),
    }
}

fn bundle_in(config: &Path) -> PathBuf {
    config.join("solstone-observer").join("spl").join("laptop")
}

#[test]
fn a_peer_claiming_another_journals_id_is_refused_and_a_genuine_journal_pairs() {
    let genuine = journal();
    let genuine_jid = genuine.jid.clone();
    let (word1, word2) = words_of(&genuine_jid);
    let right_words = format!("{word1} {word2}");
    let temp = tempfile::tempdir().expect("temp dir");

    // Its own CA and a valid pin to it, the real journal's id and that journal's
    // mark words: the owner would read the mark they know and say yes.
    let forger = open_door(journal(), &genuine_jid);
    let output = join(temp.path(), &forger.link, &right_words);
    assert_eq!(output.exit, 1, "{}{}", output.stdout, output.stderr);
    assert!(!bundle_in(temp.path()).exists());
    assert_eq!(forger.requests.lock().expect("requests").len(), 1);
    forger.runtime.shutdown_background();

    let door = open_door(genuine, &genuine_jid);
    let output = join(temp.path(), &door.link, &right_words);
    assert_eq!(output.exit, 0, "{}{}", output.stdout, output.stderr);
    let peer: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle_in(temp.path()).join("peer.json")).expect("peer.json"),
    )
    .expect("peer json");
    assert_eq!(peer["instance_id"], genuine_jid);
    door.runtime.shutdown_background();
}

fn confirm(config: &Path, mark: &str) -> CommandOutput {
    let env = BTreeMap::from([
        ("XDG_CONFIG_HOME".to_string(), config.display().to_string()),
        ("HOME".to_string(), config.display().to_string()),
    ]);
    let args = ["link", "confirm", "--label", "laptop", "--mark", mark].map(str::to_owned);
    let transport = ScriptedHttpTransport::new(vec![]);
    let seams = LinkDispatchSeams {
        transport: &transport,
        clock: None,
        files: None,
        link_pairing: Some(&SplLinkJoinPairingSeam),
        link_serve: None,
        link_status_probe: None,
    };
    match dispatch_sol_link_with_seams(&args, &env, "", seams) {
        LinkDispatch::Buffered(output) => output,
        LinkDispatch::Resident { .. } => panic!("link confirm must stay buffered"),
    }
}

/// `link serve` as it starts: refused here, it never reaches the carrier.
fn serve_refusal(config: &Path) -> Option<CommandOutput> {
    let env = BTreeMap::from([
        ("XDG_CONFIG_HOME".to_string(), config.display().to_string()),
        ("HOME".to_string(), config.display().to_string()),
    ]);
    let args = ["link", "serve", "--label", "laptop"].map(str::to_owned);
    let transport = ScriptedHttpTransport::new(vec![]);
    let seams = LinkDispatchSeams {
        transport: &transport,
        clock: None,
        files: None,
        link_pairing: None,
        link_serve: None,
        link_status_probe: None,
    };
    let LinkDispatch::Resident { handler, args } =
        dispatch_sol_link_with_seams(&args, &env, "", seams)
    else {
        panic!("link serve is resident");
    };
    handler(solstone_core_sol_client::command::CommandContext {
        args: &args,
        env: &env,
        stdin: "".into(),
        transport: &transport,
        clock: None,
        files: None,
        build_identity: None,
        client_item_ids: None,
        notification_sink: None,
        link_pairing: None,
        link_serve: None,
        link_status_probe: None,
    })
    .err()
}

fn store_other_id(config: &Path, other: &str) {
    let path = bundle_in(config).join("peer.json");
    let mut peer: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("peer.json")).expect("peer json");
    peer["instance_id"] = serde_json::json!(other);
    std::fs::write(&path, peer.to_string()).expect("peer.json");
}

#[test]
fn a_stored_id_that_is_not_its_cas_is_held_and_asked_with_the_cas_mark() {
    let genuine = journal();
    let jid = genuine.jid.clone();
    let (word1, word2) = words_of(&jid);
    let temp = tempfile::tempdir().expect("temp dir");
    let door = open_door(genuine, &jid);
    let joined = join(temp.path(), &door.link, &format!("{word1} {word2}"));
    assert_eq!(joined.exit, 0, "{}{}", joined.stdout, joined.stderr);

    // A healthy link isn't asked again, and serve goes past the hold.
    assert_eq!(confirm(temp.path(), "any words").exit, 0);
    let past_hold = serve_refusal(temp.path()).expect("no serve seam here");
    assert_eq!(past_hold.exit, 1, "{}", past_hold.stderr);

    let other = journal().jid;
    store_other_id(temp.path(), &other);
    let held = serve_refusal(temp.path()).expect("held");
    assert_eq!(held.exit, 5, "{}{}", held.stdout, held.stderr);

    // Yes with the CA's words stores the CA's id and lets it go.
    let answered = confirm(temp.path(), &format!("{word1} {word2}"));
    assert_eq!(answered.exit, 0, "{}{}", answered.stdout, answered.stderr);
    let peer: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle_in(temp.path()).join("peer.json")).expect("peer.json"),
    )
    .expect("peer json");
    assert_eq!(peer["instance_id"], jid);
    assert_eq!(
        serve_refusal(temp.path()).expect("no serve seam here").exit,
        1
    );

    // Words from the stored id are not the CA's: the link is torn down.
    store_other_id(temp.path(), &other);
    let (other1, other2) = words_of(&other);
    let refused = confirm(temp.path(), &format!("{other1} {other2}"));
    assert_eq!(refused.exit, 1, "{}{}", refused.stdout, refused.stderr);
    assert!(!bundle_in(temp.path()).exists());
    assert!(
        door.requests
            .lock()
            .expect("requests")
            .iter()
            .any(|request| request.starts_with("DELETE /app/network/api/clients/sha256:"))
    );
    door.runtime.shutdown_background();
}

/// A scripted terminal in front of the real check of a stored chain.
struct TerminalOnSplCheck(ScriptedLinkJoinPairingSeam);

impl LinkJoinPairingSeam for TerminalOnSplCheck {
    fn pair_direct(
        &self,
        request: LinkJoinDirectRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError> {
        self.0.pair_direct(request)
    }

    fn pair_relay(
        &self,
        request: LinkJoinRelayRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError> {
        self.0.pair_relay(request)
    }

    fn open_terminal(&self) -> Option<Box<dyn LinkJoinTerminal>> {
        self.0.open_terminal()
    }

    fn retire(&self, credential: &LinkJoinCredential) {
        self.0.retire(credential);
    }

    fn pinned_ca_jid(&self, ca_chain_pem: &[String]) -> Option<String> {
        SplLinkJoinPairingSeam.pinned_ca_jid(ca_chain_pem)
    }
}

/// `link serve` through the real runner's check of the chain. Only called where the
/// hold stops it, so no carrier starts.
fn serve_with_real_check(config: &Path) -> CommandOutput {
    let env = BTreeMap::from([
        ("XDG_CONFIG_HOME".to_string(), config.display().to_string()),
        ("HOME".to_string(), config.display().to_string()),
    ]);
    let args = ["link", "serve", "--label", "laptop"].map(str::to_owned);
    let transport = ScriptedHttpTransport::new(vec![]);
    let runner = solstone_core_sol_link::SplLinkServeRunner;
    let seams = LinkDispatchSeams {
        transport: &transport,
        clock: None,
        files: None,
        link_pairing: None,
        link_serve: Some(&runner),
        link_status_probe: None,
    };
    let LinkDispatch::Resident { handler, args } =
        dispatch_sol_link_with_seams(&args, &env, "", seams)
    else {
        panic!("link serve is resident");
    };
    let refused = handler(solstone_core_sol_client::command::CommandContext {
        args: &args,
        env: &env,
        stdin: "".into(),
        transport: &transport,
        clock: None,
        files: None,
        build_identity: None,
        client_item_ids: None,
        notification_sink: None,
        link_pairing: None,
        link_serve: Some(&runner),
        link_status_probe: None,
    });
    match refused {
        Err(output) => output,
        Ok(_) => panic!("the hold should have stopped serve"),
    }
}

fn confirm_at_terminal(config: &Path, seam: &dyn LinkJoinPairingSeam) -> CommandOutput {
    let env = BTreeMap::from([
        ("XDG_CONFIG_HOME".to_string(), config.display().to_string()),
        ("HOME".to_string(), config.display().to_string()),
    ]);
    let args = ["link", "confirm", "--label", "laptop"].map(str::to_owned);
    let transport = ScriptedHttpTransport::new(vec![]);
    let seams = LinkDispatchSeams {
        transport: &transport,
        clock: None,
        files: None,
        link_pairing: Some(seam),
        link_serve: None,
        link_status_probe: None,
    };
    match dispatch_sol_link_with_seams(&args, &env, "", seams) {
        LinkDispatch::Buffered(output) => output,
        LinkDispatch::Resident { .. } => panic!("link confirm must stay buffered"),
    }
}

#[test]
fn the_terminal_is_asked_about_the_cas_mark_and_no_tears_the_link_down() {
    let genuine = journal();
    let jid = genuine.jid.clone();
    let (word1, word2) = words_of(&jid);
    let temp = tempfile::tempdir().expect("temp dir");
    let door = open_door(genuine, &jid);
    let joined = join(temp.path(), &door.link, &format!("{word1} {word2}"));
    assert_eq!(joined.exit, 0, "{}{}", joined.stdout, joined.stderr);
    let other = journal().jid;
    store_other_id(temp.path(), &other);
    let (other1, other2) = words_of(&other);

    // No terminal and no words: it says what to do and changes nothing.
    let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
    assert_eq!(confirm_at_terminal(temp.path(), &seam).exit, 1);
    assert!(bundle_in(temp.path()).exists());

    // Walking away keeps it held.
    let seam = ScriptedLinkJoinPairingSeam::new(vec![]).with_terminal(&[]);
    assert_eq!(confirm_at_terminal(temp.path(), &seam).exit, 5);
    assert!(bundle_in(temp.path()).exists());
    assert!(seam.retired().is_empty());
    let shown = seam.terminal_shown();
    assert!(shown.contains(&format!("{word1}\u{b7}{word2}")), "{shown}");
    assert!(
        !shown.contains(&format!("{other1}\u{b7}{other2}")),
        "{shown}"
    );

    // No retires the pairing and takes the link away.
    let seam = ScriptedLinkJoinPairingSeam::new(vec![]).with_terminal(&["no"]);
    assert_eq!(confirm_at_terminal(temp.path(), &seam).exit, 1);
    assert!(!bundle_in(temp.path()).exists());
    assert_eq!(seam.retired(), vec![other]);
    door.runtime.shutdown_background();
}

/// Chains that can't show what a link pins are held and name no mark: a head that
/// won't parse, and a copy of the owner's key under someone else's signature.
#[test]
fn a_stored_link_with_no_readable_ca_is_held_and_shows_no_mark_from_its_id() {
    let forger = journal();
    let owner_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("owner key");
    let owner = CertificateParams::new(Vec::<String>::new())
        .expect("params")
        .self_signed(&owner_key)
        .expect("owner CA");
    let owner_spki = spl_core::ca::extract_spki_der(owner.der()).expect("SPKI");
    let owner_jid = spl_core::relay_window::jid_from_spki(&owner_spki).expect("id");
    let (owner1, owner2) = words_of(&owner_jid);
    let copied = CertificateParams::new(Vec::<String>::new())
        .expect("params")
        .signed_by(&owner_key, &forger.cert, &forger.key)
        .expect("copied key");

    for (name, chain) in [
        (
            "unreadable",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_string(),
        ),
        ("copied-key", copied.pem()),
    ] {
        let temp = tempfile::tempdir().expect("temp dir");
        let paired = journal();
        let paired_jid = paired.jid.clone();
        let (word1, word2) = words_of(&paired_jid);
        let door = open_door(paired, &paired_jid);
        let joined = join(temp.path(), &door.link, &format!("{word1} {word2}"));
        assert_eq!(joined.exit, 0, "{}{}", joined.stdout, joined.stderr);
        store_other_id(temp.path(), &owner_jid);
        std::fs::write(bundle_in(temp.path()).join("chain.pem"), &chain).expect("chain.pem");

        // Serve stops at the real runner's check of the chain.
        let held = serve_with_real_check(temp.path());
        assert_eq!(held.exit, 5, "{name}: {}{}", held.stdout, held.stderr);

        // Words from the claimed id can't confirm it, and it stays.
        let answered = confirm(temp.path(), &format!("{owner1} {owner2}"));
        assert_eq!(
            answered.exit, 5,
            "{name}: {}{}",
            answered.stdout, answered.stderr
        );
        assert!(bundle_in(temp.path()).exists(), "{name}");

        // At the terminal it's the couldn't-verify question, with no mark from the id.
        let seam = TerminalOnSplCheck(
            ScriptedLinkJoinPairingSeam::new(vec![]).with_terminal(&["continue"]),
        );
        assert_eq!(confirm_at_terminal(temp.path(), &seam).exit, 0, "{name}");
        let shown = seam.0.terminal_shown();
        assert!(
            !shown.contains(&format!("{owner1}\u{b7}{owner2}")),
            "{name}: {shown}"
        );

        // Going on is remembered.
        assert_eq!(confirm(temp.path(), "any words").exit, 0, "{name}");

        // Cancelling instead takes the link away.
        let peer_path = bundle_in(temp.path()).join("peer.json");
        store_other_id(temp.path(), &owner_jid);
        let mut peer: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&peer_path).expect("peer")).expect("peer json");
        peer.as_object_mut()
            .expect("object")
            .remove("unverified_ca_accepted");
        std::fs::write(&peer_path, peer.to_string()).expect("peer");
        let seam =
            TerminalOnSplCheck(ScriptedLinkJoinPairingSeam::new(vec![]).with_terminal(&["cancel"]));
        assert_eq!(confirm_at_terminal(temp.path(), &seam).exit, 1, "{name}");
        assert!(!bundle_in(temp.path()).exists(), "{name}");
        door.runtime.shutdown_background();
    }
}

#[test]
fn wrong_mark_words_leave_nothing_saved_and_ask_the_journal_to_forget_the_device() {
    let genuine = journal();
    let jid = genuine.jid.clone();
    let temp = tempfile::tempdir().expect("temp dir");
    let door = open_door(genuine, &jid);

    let output = join(temp.path(), &door.link, "wrong words");

    assert_eq!(output.exit, 1, "{}{}", output.stdout, output.stderr);
    assert!(!bundle_in(temp.path()).exists());
    let requests = door.requests.lock().expect("requests").clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests[1].starts_with("DELETE /app/network/api/clients/sha256:"),
        "{requests:?}"
    );
    door.runtime.shutdown_background();
}
