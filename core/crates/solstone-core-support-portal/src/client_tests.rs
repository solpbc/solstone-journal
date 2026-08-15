// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::io::Cursor;
use std::path::Path;

use base64::Engine;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use rsa::pkcs8::EncodePublicKey;
use tempfile::TempDir;

use crate::client::{
    MultipartInput, PortalClient, PortalResponse, PortalRuntime, is_enabled,
    portal_url_from_settings, portal_url_from_settings_with_env,
};
use crate::errors::PortalClientError;
use crate::fake_portal::{HttpReply, LoopbackPortal, StubTransport};
use crate::{
    dpop::create_dpop_proof,
    jwk::thumbprint,
    keypair::Keypair,
    token::{create_access_token, sign_tos},
};

struct FixedRuntime {
    now: i64,
    ids: VecDeque<String>,
    bytes: VecDeque<[u8; 4]>,
}
impl FixedRuntime {
    fn new() -> Self {
        Self {
            now: 1_767_225_600,
            ids: std::iter::repeat_n("00000000-0000-4000-8000-000000000000".to_owned(), 32)
                .collect(),
            bytes: VecDeque::from([[0, 1, 2, 3]; 32]),
        }
    }
}
impl PortalRuntime for FixedRuntime {
    fn now(&mut self) -> i64 {
        self.now
    }
    fn uuid(&mut self) -> String {
        self.ids
            .pop_front()
            .unwrap_or_else(|| "00000000-0000-4000-8000-000000000000".to_owned())
    }
    fn random_bytes(&mut self, bytes: &mut [u8]) -> Result<(), PortalClientError> {
        bytes.copy_from_slice(&self.bytes.pop_front().unwrap_or([0, 1, 2, 3]));
        Ok(())
    }
}

fn response(status: u16, body: &str) -> PortalResponse {
    PortalResponse {
        status,
        body: body.to_owned(),
    }
}
fn client(
    dir: &Path,
    replies: Vec<PortalResponse>,
) -> (
    PortalClient,
    std::sync::Arc<std::sync::Mutex<Vec<crate::fake_portal::RequestLog>>>,
) {
    let (transport, log) = StubTransport::new("https://portal.example", replies);
    (
        PortalClient::new_with(
            "https://portal.example",
            dir,
            Some("test-abcd".to_owned()),
            false,
            Box::new(transport),
            Box::new(FixedRuntime::new()),
        )
        .unwrap(),
        log,
    )
}

#[test]
fn ensure_registered_is_guarded_but_register_always_writes() {
    let dir = TempDir::new().unwrap();
    let (mut client, log) = client(
        dir.path(),
        vec![
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
            response(200, "terms"),
            response(200, r#"{"access_token":"token2"}"#),
        ],
    );
    client.ensure_registered().unwrap();
    let first = log.lock().unwrap().len();
    client.ensure_registered().unwrap();
    assert_eq!(log.lock().unwrap().len(), first);
    client.register().unwrap();
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.path == "/api/signup")
            .count(),
        2
    );
}

#[test]
fn collision_has_exactly_four_signup_attempts() {
    let dir = TempDir::new().unwrap();
    let mut replies = Vec::new();
    for _ in 0..4 {
        replies.push(response(200, "terms"));
        replies.push(response(409, "collision"));
    }
    let (mut client, log) = client(dir.path(), replies);
    assert!(matches!(
        client.register(),
        Err(PortalClientError::HandleCollision)
    ));
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.path == "/api/signup")
            .count(),
        4
    );
}

#[test]
fn tos_changed_retries_once_and_rewinds_multipart() {
    let dir = TempDir::new().unwrap();
    let (transport, log) = StubTransport::new(
        "https://portal.example",
        vec![
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
            response(401, r#"{"error":"tos_changed"}"#),
            response(200, "terms2"),
            response(200, r#"{"access_token":"token2"}"#),
            response(200, "done"),
        ],
    );
    let multipart_bodies = transport.multipart_bodies.clone();
    let mut client = PortalClient::new_with(
        "https://portal.example",
        dir.path(),
        Some("test-abcd".to_owned()),
        false,
        Box::new(transport),
        Box::new(FixedRuntime::new()),
    )
    .unwrap();
    client.register().unwrap();
    let mut file = Cursor::new(b"payload".to_vec());
    file.set_position(7);
    let mut files = [MultipartInput {
        name: "file".to_owned(),
        filename: "file.txt".to_owned(),
        content_type: None,
        reader: &mut file,
    }];
    assert_eq!(
        client
            .authed_request(
                "POST",
                "https://portal.example/api/upload",
                None,
                None,
                Some(&mut files),
                None
            )
            .unwrap()
            .body,
        "done"
    );
    assert_eq!(
        *multipart_bodies.lock().unwrap(),
        vec![b"payload".to_vec(), b"payload".to_vec()]
    );
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.path == "/api/upload")
            .count(),
        2
    );
}

#[test]
fn handle_memoizes_hostname_and_collision_replaces_old_suffix() {
    let dir = TempDir::new().unwrap();
    let (transport, _) = StubTransport::new("https://portal.example", vec![]);
    let mut derived = PortalClient::new_with(
        "https://portal.example",
        dir.path(),
        None,
        false,
        Box::new(transport),
        Box::new(FixedRuntime::new()),
    )
    .unwrap();
    let first = derived.handle().to_owned();
    assert_eq!(derived.handle(), first);

    let (transport, log) = StubTransport::new(
        "https://portal.example",
        vec![
            response(200, "terms"),
            response(409, "collision"),
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
        ],
    );
    let mut runtime = FixedRuntime::new();
    runtime.bytes = VecDeque::from([[4, 5, 6, 7]]);
    let mut client = PortalClient::new_with(
        "https://portal.example",
        dir.path().join("retry"),
        Some("test-abcd".to_owned()),
        false,
        Box::new(transport),
        Box::new(runtime),
    )
    .unwrap();
    client.register().unwrap();
    let signups: Vec<_> = log
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.path == "/api/signup")
        .map(|request| {
            serde_json::from_str::<serde_json::Value>(request.body.as_deref().unwrap()).unwrap()
        })
        .collect();
    assert_eq!(signups[1]["handle"], "test-efgh");
    assert_ne!(signups[1]["handle"], "test-abcd-efgh");
}

#[test]
fn settings_fail_open() {
    let dir = TempDir::new().unwrap();
    assert!(is_enabled(dir.path()));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/config.json"), "not json").unwrap();
    assert!(is_enabled(dir.path()));
    assert_eq!(
        portal_url_from_settings(dir.path()),
        "https://support.solstone.app"
    );
}

#[test]
fn settings_precedence_empty_env_and_enabled_fail_open_cases() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/config.json"),
        r#"{"support":{"portal_url":"https://config.example///","enabled":false}}"#,
    )
    .unwrap();
    assert_eq!(
        portal_url_from_settings_with_env(dir.path(), Some("")),
        "https://config.example"
    );
    assert_eq!(
        portal_url_from_settings_with_env(dir.path(), Some("https://env.example///")),
        "https://env.example"
    );
    std::fs::write(
        dir.path().join("config/config.json"),
        r#"{"support":{"portal_url":"","enabled":0}}"#,
    )
    .unwrap();
    assert_eq!(
        portal_url_from_settings_with_env(dir.path(), None),
        "https://support.solstone.app"
    );
    assert!(!is_enabled(dir.path()));
    for value in ["null", "false", "\"\"", "[]", "{}"] {
        std::fs::write(
            dir.path().join("config/config.json"),
            format!(r#"{{"support":{{"enabled":{value}}}}}"#),
        )
        .unwrap();
        assert!(!is_enabled(dir.path()));
    }
    std::fs::write(
        dir.path().join("config/config.json"),
        r#"{"support":{"enabled":"yes"}}"#,
    )
    .unwrap();
    assert!(is_enabled(dir.path()));
    assert!(!is_enabled(dir.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("config/config.json"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        if std::fs::read_to_string(dir.path().join("config/config.json")).is_err() {
            assert!(is_enabled(dir.path()));
        }
    }
}

#[test]
fn authed_request_preserves_delete_params_idempotency_and_dpop_htu() {
    let dir = TempDir::new().unwrap();
    let (mut client, log) = client(
        dir.path(),
        vec![
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
            response(200, "done"),
        ],
    );
    client.register().unwrap();
    let params = [("status".to_owned(), "open".to_owned())];
    client
        .authed_request(
            "DELETE",
            "/api/tickets",
            None,
            Some(&params),
            None,
            Some("idem-1"),
        )
        .unwrap();
    let request = log.lock().unwrap().last().unwrap().clone();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path, "/api/tickets?status=open");
    assert_eq!(
        request
            .headers
            .iter()
            .find(|(name, _)| name == "Idempotency-Key")
            .map(|(_, value)| value.as_str()),
        Some("idem-1")
    );
    let proof = request
        .headers
        .iter()
        .find(|(name, _)| name == "DPoP")
        .map(|(_, value)| value)
        .unwrap();
    assert_eq!(
        proof_payload(proof)["htu"],
        "https://portal.example/api/tickets"
    );
}

#[test]
fn non_ascii_jwt_and_signup_json_use_python_ascii_escapes() {
    let keypair = Keypair::from_pem(include_bytes!(
        "../../../fixtures/support_portal_golden_nonproduction/keypair.pem"
    ))
    .unwrap();
    let token = create_access_token(
        &keypair.signer,
        "terms",
        "https://portal.é",
        &keypair.thumbprint,
        "id",
        1,
    )
    .unwrap();
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1).unwrap())
        .unwrap();
    assert!(payload.windows(6).any(|bytes| bytes == b"\\u00e9"));
    assert!(payload.iter().all(|byte| byte.is_ascii()));

    let dir = TempDir::new().unwrap();
    let (transport, log) = StubTransport::new(
        "https://portal.é",
        vec![
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
        ],
    );
    let mut client = PortalClient::new_with(
        "https://portal.é",
        dir.path(),
        Some("h-é".to_owned()),
        false,
        Box::new(transport),
        Box::new(FixedRuntime::new()),
    )
    .unwrap();
    client.register().unwrap();
    let body = log
        .lock()
        .unwrap()
        .iter()
        .find(|request| request.path == "/api/signup")
        .unwrap()
        .body
        .as_deref()
        .unwrap()
        .to_owned();
    assert!(body.as_bytes().windows(6).any(|bytes| bytes == b"\\u00e9"));
    assert!(body.as_bytes().iter().all(|byte| byte.is_ascii()));
}

#[test]
fn empty_and_null_handles_follow_python_truthiness_and_get_semantics() {
    let dir = TempDir::new().unwrap();
    let (transport, _) = StubTransport::new("https://portal.example", vec![]);
    let mut empty = PortalClient::new_with(
        "https://portal.example",
        dir.path(),
        Some(String::new()),
        true,
        Box::new(transport),
        Box::new(FixedRuntime::new()),
    )
    .unwrap();
    assert!(empty.handle().starts_with("anon-"));
    let (transport, _) = StubTransport::new("https://portal.example", vec![]);
    std::fs::write(
        dir.path().join("token.json"),
        r#"{"access_token":"token","handle":null}"#,
    )
    .unwrap();
    let mut loaded = PortalClient::new_with(
        "https://portal.example",
        dir.path(),
        Some("kept".to_owned()),
        false,
        Box::new(transport),
        Box::new(FixedRuntime::new()),
    )
    .unwrap();
    assert_ne!(loaded.handle(), "kept");
}

#[test]
fn corpus_portal_request_counts_match_registration_expectations() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/convey_support_corpus.json")).unwrap();
    for (phase, expected) in [("established", 1), ("unregistered", 3)] {
        let case = corpus["phases"][phase]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "api_announcements")
            .unwrap();
        assert_eq!(case["portal_requests"].as_array().unwrap().len(), expected);
    }
}

fn golden() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/support_portal_golden_nonproduction/vectors.json"
    ))
    .unwrap()
}

fn proof_payload(proof: &str) -> serde_json::Value {
    let payload = proof.split('.').nth(1).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn verify_golden_proof(vector: &serde_json::Value, proof: &str) {
    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(vector["jwk"]["n"].as_str().unwrap())
        .unwrap();
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(vector["jwk"]["e"].as_str().unwrap())
        .unwrap();
    let key = rsa::RsaPublicKey::new(
        rsa::BigUint::from_bytes_be(&n),
        rsa::BigUint::from_bytes_be(&e),
    )
    .unwrap();
    let der = key.to_public_key_der().unwrap();
    let (input, signature) = proof.rsplit_once('.').unwrap();
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .unwrap();
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, der.as_ref())
        .verify(input.as_bytes(), &signature)
        .unwrap();
}

#[test]
fn golden_key_and_wire_vectors_are_byte_exact() {
    let vector = golden();
    let pem = include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem");
    let keypair = Keypair::from_pem(pem).unwrap();
    assert_eq!(serde_json::to_value(&keypair.jwk).unwrap(), vector["jwk"]);
    assert_eq!(thumbprint(&keypair.jwk), vector["jwk_thumbprint"]);
    assert_eq!(
        sign_tos(
            &keypair.signer,
            vector["signature_interop"]["probe"].as_str().unwrap()
        )
        .unwrap(),
        vector["signature_interop"]["signature_b64url"]
    );
    let pinned = &vector["pinned"];
    assert_eq!(
        create_access_token(
            &keypair.signer,
            pinned["tos_text"].as_str().unwrap(),
            pinned["portal_url"].as_str().unwrap(),
            vector["jwk_thumbprint"].as_str().unwrap(),
            pinned["jti"].as_str().unwrap(),
            pinned["iat"].as_i64().unwrap()
        )
        .unwrap(),
        vector["access_token"]
    );
}

#[test]
fn golden_storage_loads_identity_token_and_verifies_recorded_proof() {
    let vector = golden();
    let storage = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/support_portal_golden_nonproduction");
    let (transport, _) = StubTransport::new("https://support.example.invalid", vec![]);
    let client = PortalClient::new_with(
        vector["pinned"]["portal_url"].as_str().unwrap(),
        storage,
        None,
        false,
        Box::new(transport),
        Box::new(FixedRuntime::new()),
    )
    .unwrap();
    assert!(client.is_registered());
    assert_eq!(
        client.access_token(),
        Some(vector["access_token"].as_str().unwrap())
    );
    verify_golden_proof(
        &vector,
        vector["dpop"]["proof_with_access_token"].as_str().unwrap(),
    );
}

#[test]
fn dpop_golden_proofs_prove_htu_and_ath_ordering() {
    let vector = golden();
    let keypair = Keypair::from_pem(include_bytes!(
        "../../../fixtures/support_portal_golden_nonproduction/keypair.pem"
    ))
    .unwrap();
    let pinned = &vector["pinned"];
    let dpop = &vector["dpop"];
    assert_eq!(
        create_dpop_proof(
            &keypair.signer,
            &keypair.jwk,
            dpop["method"].as_str().unwrap(),
            dpop["url_with_query_and_fragment"].as_str().unwrap(),
            pinned["jti"].as_str().unwrap(),
            pinned["iat"].as_i64().unwrap(),
            Some(vector["access_token"].as_str().unwrap())
        )
        .unwrap(),
        dpop["proof_with_access_token"]
    );
    assert_eq!(
        create_dpop_proof(
            &keypair.signer,
            &keypair.jwk,
            dpop["method"].as_str().unwrap(),
            dpop["url_fragment_only"].as_str().unwrap(),
            pinned["jti"].as_str().unwrap(),
            pinned["iat"].as_i64().unwrap(),
            None
        )
        .unwrap(),
        dpop["proof_fragment_only"]
    );
}

#[test]
fn recorded_dpop_payloads_discriminate_htu_and_ath() {
    let vector = golden();
    let dpop = &vector["dpop"];
    let with_access = proof_payload(dpop["proof_with_access_token"].as_str().unwrap());
    let without_access = proof_payload(dpop["proof_without_access_token"].as_str().unwrap());
    let fragment = proof_payload(dpop["proof_fragment_only"].as_str().unwrap());
    assert_eq!(
        with_access["htu"],
        dpop["expected_htu_from_query_and_fragment"]
    );
    assert_eq!(fragment["htu"], dpop["expected_htu_from_fragment_only"]);
    assert_eq!(with_access["htu"], without_access["htu"]);
    assert_eq!(with_access["ath"], dpop["ath_b64url"]);
    assert!(without_access.get("ath").is_none());
}

#[test]
fn generated_key_round_trips_into_ring() {
    let (generated, pem) = Keypair::generate().unwrap();
    let loaded = Keypair::from_pem(&pem).unwrap();
    assert_eq!(
        sign_tos(&generated.signer, "generated probe").unwrap(),
        sign_tos(&loaded.signer, "generated probe").unwrap()
    );
}

#[test]
fn principal_reuses_existing_keypair_without_rewriting_it() {
    let dir = TempDir::new().unwrap();
    let original =
        include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem");
    std::fs::write(dir.path().join("keypair.pem"), original).unwrap();
    let (mut client, _) = client(dir.path(), vec![]);
    assert_eq!(
        client.principal().unwrap(),
        "jkt:A-MnX88Mi4pSXKB4-YeSQv1U9-eZL59r6zji3eEUUqI"
    );
    assert_eq!(
        std::fs::read(dir.path().join("keypair.pem")).unwrap(),
        original
    );
}

#[test]
fn second_tos_changed_is_returned_without_a_second_reregistration() {
    let dir = TempDir::new().unwrap();
    let (mut client, log) = client(
        dir.path(),
        vec![
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
            response(401, r#"{"error":"tos_changed"}"#),
            response(200, "terms2"),
            response(200, r#"{"access_token":"token2"}"#),
            response(401, r#"{"error":"tos_changed"}"#),
        ],
    );
    client.register().unwrap();
    assert_eq!(
        client
            .authed_request("GET", "/api/read", None, None, None, None)
            .unwrap()
            .status,
        401
    );
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.path == "/api/read")
            .count(),
        2
    );
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.path == "/api/signup")
            .count(),
        2
    );
}

#[test]
fn unrelated_or_non_json_401_does_not_reregister() {
    for body in ["not json", r#"{"error":"other"}"#] {
        let dir = TempDir::new().unwrap();
        let (mut client, log) = client(
            dir.path(),
            vec![
                response(200, "terms"),
                response(200, r#"{"access_token":"token"}"#),
                response(401, body),
            ],
        );
        client.register().unwrap();
        assert_eq!(
            client
                .authed_request("GET", "/api/read", None, None, None, None)
                .unwrap()
                .status,
            401
        );
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.path == "/api/signup")
                .count(),
            1
        );
    }
}

#[test]
fn anonymous_state_is_never_persisted() {
    let dir = TempDir::new().unwrap();
    let (transport, _) = StubTransport::new(
        "https://portal.example",
        vec![
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
        ],
    );
    let mut client = PortalClient::new_with(
        "https://portal.example",
        dir.path(),
        None,
        true,
        Box::new(transport),
        Box::new(FixedRuntime::new()),
    )
    .unwrap();
    client.register().unwrap();
    assert!(!dir.path().join("keypair.pem").exists());
    assert!(!dir.path().join("token.json").exists());
    assert!(!dir.path().join("tos.txt").exists());
}

#[test]
fn state_load_keeps_reference_asymmetry() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("token.json"), "not json").unwrap();
    std::fs::write(dir.path().join("tos.txt"), "cached").unwrap();
    let (client, _) = client(dir.path(), vec![]);
    assert_eq!(client.cached_tos(), Some("cached"));
    std::fs::write(dir.path().join("keypair.pem"), "not a pem").unwrap();
    let (transport, _) = StubTransport::new("https://portal.example", vec![]);
    assert!(matches!(
        PortalClient::new_with(
            "https://portal.example",
            dir.path(),
            None,
            false,
            Box::new(transport),
            Box::new(FixedRuntime::new())
        ),
        Err(PortalClientError::KeypairInvalid { .. })
    ));
}

#[test]
fn status_error_includes_only_the_first_500_characters() {
    let dir = TempDir::new().unwrap();
    let (client, _) = client(dir.path(), vec![]);
    let body = "x".repeat(501);
    let error = client
        .raise_for_status(
            "GET",
            "https://portal.example/too-long",
            &response(500, &body),
        )
        .unwrap_err();
    let PortalClientError::HttpStatus { message } = error else {
        panic!("wrong error")
    };
    assert!(message.ends_with(&"x".repeat(500)));
    assert_eq!(message.matches('x').count(), 500);
    assert!(message.starts_with("GET https://portal.example/too-long — 500: "));
}

#[test]
fn real_transport_does_not_follow_redirects_and_sends_dpop_headers() {
    let target = LoopbackPortal::new(vec![]);
    let redirect = LoopbackPortal::new(vec![HttpReply {
        status: 302,
        headers: vec![("Location".to_owned(), format!("{}/reached", target.url()))],
        body: String::new(),
    }]);
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("keypair.pem"),
        include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("token.json"),
        r#"{"access_token":"loop-token"}"#,
    )
    .unwrap();
    let mut client =
        PortalClient::new(redirect.url(), dir.path(), Some("loop".to_owned()), false).unwrap();
    let error = client
        .authed_request("GET", "/read", None, None, None, None)
        .unwrap_err();
    assert!(
        matches!(error, PortalClientError::HttpStatus { ref message } if message.contains(" — 302: "))
    );
    let requests = redirect.log();
    let request = requests.lock().unwrap().pop().unwrap();
    assert_eq!(request.path, "/read");
    assert_eq!(
        request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.as_str()),
        Some("DPoP loop-token")
    );
    let proof = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("dpop"))
        .map(|(_, value)| value)
        .unwrap();
    assert_eq!(
        proof_payload(proof)["htu"],
        format!("{}/read", redirect.url())
    );
    assert!(target.log().lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn keypair_is_0600_but_token_uses_process_umask() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let (mut client, _) = client(
        dir.path(),
        vec![
            response(200, "terms"),
            response(200, r#"{"access_token":"token"}"#),
        ],
    );
    client.principal().unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("keypair.pem"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    client.register().unwrap();
    std::fs::File::create(dir.path().join("control")).unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("token.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        std::fs::metadata(dir.path().join("control"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    );
}

#[cfg(unix)]
#[test]
fn saving_over_existing_keypair_repairs_its_mode() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("keypair.pem");
    let pem = include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem");
    std::fs::write(&path, pem).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    crate::keypair::save_keypair(&path, pem).unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
