// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fail-closed parsing of nvattest stdout and its GPU claims.

use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use serde_json::{Map, Value};

use crate::{
    error::{GpuAppraisalReason, GpuClaimsError},
    snp::AppraisalStep,
    tlv::GpuEnvelope,
};

const REPORT_TRUE_KEYS: [&str; 5] = [
    "x-nvidia-gpu-attestation-report-parsed",
    "x-nvidia-gpu-attestation-report-signature-verified",
    "x-nvidia-gpu-attestation-report-nonce-match",
    "x-nvidia-gpu-attestation-report-cert-chain-fwid-match",
    "x-nvidia-gpu-arch-check",
];
const DRIVER_RIM_TRUE_KEYS: [&str; 3] = [
    "x-nvidia-gpu-driver-rim-signature-verified",
    "x-nvidia-gpu-driver-rim-version-match",
    "x-nvidia-gpu-driver-rim-measurements-available",
];
const VBIOS_RIM_TRUE_KEYS: [&str; 4] = [
    "x-nvidia-gpu-vbios-rim-signature-verified",
    "x-nvidia-gpu-vbios-rim-version-match",
    "x-nvidia-gpu-vbios-rim-measurements-available",
    "x-nvidia-gpu-vbios-index-no-conflict",
];
const CERT_CHAIN_KEYS: [&str; 3] = [
    "x-nvidia-gpu-attestation-report-cert-chain",
    "x-nvidia-gpu-driver-rim-cert-chain",
    "x-nvidia-gpu-vbios-rim-cert-chain",
];

/// A successfully accepted nvattest claim object.
#[derive(Debug, Clone, PartialEq)]
pub struct NvattestAcceptance {
    pub claim: Map<String, Value>,
}

/// A fail-closed nvattest rejection with a stable reason and failed check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvattestRejection {
    pub reason: GpuAppraisalReason,
    pub check: GpuClaimsError,
}

/// The accepted or rejected result of pure nvattest stdout appraisal.
#[derive(Debug, Clone, PartialEq)]
pub enum NvattestVerdict {
    Accepted(NvattestAcceptance),
    Rejected(NvattestRejection),
}

/// GPU provenance obtained from accepted claims and unverified SPP envelope metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuAppraisal {
    pub steps: Vec<AppraisalStep>,
    pub driver_version: String,
    pub vbios_version: String,
    pub hwmodel: String,
    pub ueid: String,
    pub oemid: String,
    pub eat_nonce: String,
    pub claims_version: String,
    pub arch: String,
    pub envelope_gpu_uuid: String,
}

/// Parses nvattest stdout as JSON without imposing an object shape.
pub fn parse_nvattest_stdout(stdout: &str) -> Result<Value, GpuClaimsError> {
    if stdout.is_empty() {
        return Err(GpuClaimsError::StdoutEmpty);
    }
    serde_json::from_str(stdout).map_err(|_| GpuClaimsError::StdoutInvalidJson)
}

/// Classifies a completed nvattest process result without consulting stderr.
pub fn classify_nvattest_result(
    returncode: i32,
    stdout: &Value,
    owner_nonce: &[u8; 32],
) -> NvattestVerdict {
    let Value::Object(stdout) = stdout else {
        return rejected(GpuClaimsError::StdoutNotObject);
    };

    let result_code = match required_stdout(stdout, "result_code") {
        Ok(value) => value,
        Err(error) => return rejected(error),
    };
    let result_message = match required_stdout(stdout, "result_message") {
        Ok(value) => value,
        Err(error) => return rejected(error),
    };
    if returncode != 0 {
        return rejected_with_reason(
            reason_for_result_code(result_code),
            GpuClaimsError::NonGreenReturncode,
        );
    }
    if !result_code_is_green(result_code) {
        return rejected_with_reason(
            reason_for_result_code(result_code),
            GpuClaimsError::NonGreenResultCode,
        );
    }
    if result_message != "Ok" {
        return rejected_with_reason(
            reason_for_result_code(result_code),
            GpuClaimsError::NonGreenResultMessage,
        );
    }

    let claims = match required_stdout(stdout, "claims") {
        Ok(value) => value,
        Err(error) => return rejected(error),
    };
    let Some([Value::Object(claim)]) = claims.as_array().map(Vec::as_slice) else {
        return rejected(GpuClaimsError::ClaimsShape);
    };
    let detached_eat = match required_stdout(stdout, "detached_eat") {
        Ok(value) => value,
        Err(error) => return rejected(error),
    };
    if let Err(error) = parse_overall_eat(detached_eat) {
        return rejected(error);
    }
    if let Err(error) = check_claim(claim, &hex_lower(owner_nonce)) {
        return rejected(error);
    }
    NvattestVerdict::Accepted(NvattestAcceptance {
        claim: claim.clone(),
    })
}

/// Builds GPU provenance from an accepted claim and SPP envelope metadata.
pub fn build_gpu_appraisal(
    claim: &Map<String, Value>,
    envelope: &GpuEnvelope,
    steps: Vec<AppraisalStep>,
) -> Result<GpuAppraisal, GpuClaimsError> {
    let arch = std::str::from_utf8(envelope.field(7).ok_or(GpuClaimsError::EnvelopeFieldUtf8)?)
        .map_err(|_| GpuClaimsError::EnvelopeFieldUtf8)?
        .to_uppercase();
    let envelope_gpu_uuid =
        std::str::from_utf8(envelope.field(6).ok_or(GpuClaimsError::EnvelopeFieldUtf8)?)
            .map_err(|_| GpuClaimsError::EnvelopeFieldUtf8)?
            .to_owned();

    Ok(GpuAppraisal {
        steps,
        driver_version: claim_string(claim, "x-nvidia-gpu-driver-version")?,
        vbios_version: claim_string(claim, "x-nvidia-gpu-vbios-version")?,
        hwmodel: claim_string(claim, "hwmodel")?,
        ueid: claim_string(claim, "ueid")?,
        oemid: claim_string(claim, "oemid")?,
        eat_nonce: claim_string(claim, "eat_nonce")?,
        claims_version: claim_string(claim, "x-nvidia-gpu-claims-version")?,
        arch,
        envelope_gpu_uuid,
    })
}

fn rejected(check: GpuClaimsError) -> NvattestVerdict {
    rejected_with_reason(GpuAppraisalReason::GpuAppraisalFailed, check)
}

fn rejected_with_reason(reason: GpuAppraisalReason, check: GpuClaimsError) -> NvattestVerdict {
    NvattestVerdict::Rejected(NvattestRejection { reason, check })
}

fn required_stdout<'a>(
    stdout: &'a Map<String, Value>,
    key: &'static str,
) -> Result<&'a Value, GpuClaimsError> {
    stdout
        .get(key)
        .ok_or(GpuClaimsError::MissingTopLevelKey { key })
}

fn required_claim<'a>(
    claim: &'a Map<String, Value>,
    key: &'static str,
) -> Result<&'a Value, GpuClaimsError> {
    claim
        .get(key)
        .ok_or(GpuClaimsError::ClaimMissingKey { key })
}

fn result_code_is_green(value: &Value) -> bool {
    matches!(value, Value::Number(number) if number.as_i64() == Some(0))
}

fn reason_for_result_code(value: &Value) -> GpuAppraisalReason {
    if matches!(value, Value::Number(number) if number.as_f64() == Some(504.0)) {
        GpuAppraisalReason::GpuNonceMismatch
    } else {
        GpuAppraisalReason::GpuAppraisalFailed
    }
}

fn parse_overall_eat(detached_eat: &Value) -> Result<(), GpuClaimsError> {
    let Some(detached_eat) = detached_eat.as_array() else {
        return Err(GpuClaimsError::DetachedEatShape);
    };
    let Some(overall) = detached_eat.first().and_then(Value::as_array) else {
        return Err(GpuClaimsError::DetachedEatShape);
    };
    let [Value::String(kind), Value::String(jwt)] = overall.as_slice() else {
        return Err(GpuClaimsError::DetachedEatShape);
    };
    if kind != "JWT" {
        return Err(GpuClaimsError::DetachedEatShape);
    }
    let mut segments = jwt.split('.');
    let (Some(header), Some(payload), Some(_signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(GpuClaimsError::JwtShape);
    };
    let header = decode_jwt_segment(header, true)?;
    let payload = decode_jwt_segment(payload, false)?;

    if header.get("alg").is_none() {
        return Err(GpuClaimsError::JwtHeaderMissingAlgorithm);
    }
    if payload.get("iss").is_none() {
        return Err(GpuClaimsError::JwtPayloadMissingIssuer);
    }
    if payload.get("x-nvidia-overall-att-result").is_none() {
        return Err(GpuClaimsError::JwtPayloadMissingOverallResult);
    }
    if header.get("alg") != Some(&Value::String("none".to_owned())) {
        return Err(GpuClaimsError::JwtAlgorithm);
    }
    if payload.get("iss") != Some(&Value::String("NVAT-LOCAL-VERIFIER".to_owned())) {
        return Err(GpuClaimsError::JwtIssuer);
    }
    if payload.get("x-nvidia-overall-att-result") != Some(&Value::Bool(true)) {
        return Err(GpuClaimsError::JwtOverallResult);
    }
    Ok(())
}

fn decode_jwt_segment(segment: &str, header: bool) -> Result<Map<String, Value>, GpuClaimsError> {
    let mut padded = segment.to_owned();
    padded.extend(std::iter::repeat_n('=', (4 - padded.len() % 4) % 4));
    let decoded = URL_SAFE
        .decode(padded)
        .map_err(|_| GpuClaimsError::JwtSegmentDecode)?;
    let value =
        serde_json::from_slice::<Value>(&decoded).map_err(|_| GpuClaimsError::JwtSegmentDecode)?;
    match value {
        Value::Object(object) => Ok(object),
        _ if header => Err(GpuClaimsError::JwtHeaderNotObject),
        _ => Err(GpuClaimsError::JwtPayloadNotObject),
    }
}

fn check_claim(claim: &Map<String, Value>, owner_nonce_hex: &str) -> Result<(), GpuClaimsError> {
    require_equal(claim, "x-nvidia-gpu-claims-version", "3.0")?;
    require_equal(claim, "x-nvidia-device-type", "gpu")?;
    require_equal(claim, "measres", "success")?;
    require_identity(claim, "secboot", &Value::Bool(true))?;
    require_equal(claim, "dbgstat", "disabled")?;
    require_equal(claim, "eat_nonce", owner_nonce_hex)?;

    for key in REPORT_TRUE_KEYS {
        require_identity(claim, key, &Value::Bool(true))?;
    }
    for key in DRIVER_RIM_TRUE_KEYS {
        require_identity(claim, key, &Value::Bool(true))?;
    }
    for key in VBIOS_RIM_TRUE_KEYS {
        require_identity(claim, key, &Value::Bool(true))?;
    }
    require_identity(claim, "x-nvidia-mismatch-measurement-records", &Value::Null)?;
    for key in CERT_CHAIN_KEYS {
        check_cert_chain(required_claim(claim, key)?)?;
    }
    Ok(())
}

fn require_equal(
    claim: &Map<String, Value>,
    key: &'static str,
    expected: &str,
) -> Result<(), GpuClaimsError> {
    if required_claim(claim, key)? != expected {
        return Err(GpuClaimsError::ClaimValueMismatch);
    }
    Ok(())
}

fn require_identity(
    claim: &Map<String, Value>,
    key: &'static str,
    expected: &Value,
) -> Result<(), GpuClaimsError> {
    if required_claim(claim, key)? != expected {
        return Err(GpuClaimsError::ClaimIdentityMismatch);
    }
    Ok(())
}

fn check_cert_chain(value: &Value) -> Result<(), GpuClaimsError> {
    let Value::Object(chain) = value else {
        return Err(GpuClaimsError::CertificateChainShape);
    };
    if chain.get("x-nvidia-cert-status") != Some(&Value::String("valid".to_owned()))
        || chain.get("x-nvidia-cert-ocsp-status") != Some(&Value::String("good".to_owned()))
        || chain.get("x-nvidia-cert-ocsp-response-valid") != Some(&Value::Bool(true))
        || chain.get("x-nvidia-cert-ocsp-nonce-matches") != Some(&Value::Bool(true))
        || chain.get("x-nvidia-cert-revocation-reason") != Some(&Value::Null)
    {
        return Err(GpuClaimsError::CertificateChainField);
    }
    Ok(())
}

fn claim_string(claim: &Map<String, Value>, key: &'static str) -> Result<String, GpuClaimsError> {
    required_claim(claim, key)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or(GpuClaimsError::ClaimStringField)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::{Map, Value, json};

    use super::{
        NvattestRejection, NvattestVerdict, build_gpu_appraisal, classify_nvattest_result,
        parse_nvattest_stdout,
    };
    use crate::{
        error::{GpuAppraisalReason, GpuClaimsError},
        test_support::fixture_bytes,
        tlv::decode_gpu_envelope,
    };

    fn owner_nonce() -> [u8; 32] {
        let hex = String::from_utf8(fixture_bytes("nonce.hex")).expect("nonce is UTF-8");
        let bytes = hex
            .split_whitespace()
            .flat_map(|line| line.as_bytes().chunks_exact(2))
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("valid hex")
            })
            .collect::<Vec<_>>();
        bytes.try_into().expect("fixture nonce is 32 bytes")
    }

    fn stdout(name: &str) -> String {
        String::from_utf8(fixture_bytes(&format!("nvattest/{name}.stdout")))
            .expect("stdout is UTF-8")
    }

    fn positive_body() -> Value {
        parse_nvattest_stdout(&stdout("positive")).expect("positive stdout parses")
    }

    fn classify(body: &Value) -> NvattestVerdict {
        classify_nvattest_result(0, body, &owner_nonce())
    }

    fn rejection(body: &Value) -> NvattestRejection {
        match classify(body) {
            NvattestVerdict::Rejected(rejection) => rejection,
            NvattestVerdict::Accepted(_) => panic!("mutation must reject"),
        }
    }

    fn claim_mut(body: &mut Value) -> &mut Map<String, Value> {
        body.as_object_mut()
            .expect("body is an object")
            .get_mut("claims")
            .expect("claims is present")
            .as_array_mut()
            .expect("claims is an array")[0]
            .as_object_mut()
            .expect("claim is an object")
    }

    fn assert_appraisal_failed(body: &Value) {
        assert_eq!(
            rejection(body).reason,
            GpuAppraisalReason::GpuAppraisalFailed
        );
    }

    fn mutate_overall_jwt(body: &mut Value, header: Option<Value>, payload: Option<Value>) {
        let jwt = body["detached_eat"][0][1]
            .as_str()
            .expect("fixture JWT")
            .to_owned();
        let parts = jwt.split('.').collect::<Vec<_>>();
        let mut decoded_header = decode_jwt_object(parts[0]);
        let mut decoded_payload = decode_jwt_object(parts[1]);
        if let Some(header) = header {
            decoded_header.extend(header.as_object().expect("header update").clone());
        }
        if let Some(payload) = payload {
            decoded_payload.extend(payload.as_object().expect("payload update").clone());
        }
        let jwt = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&decoded_header).expect("header JSON")),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&decoded_payload).expect("payload JSON")),
        );
        body["detached_eat"][0][1] = Value::String(jwt);
    }

    fn decode_jwt_object(segment: &str) -> Map<String, Value> {
        serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(segment)
                .expect("fixture JWT segment decodes"),
        )
        .expect("fixture JWT segment is JSON object")
    }

    #[test]
    fn gpu_positive_stdout_classifies_accepted() {
        let body = positive_body();
        let NvattestVerdict::Accepted(acceptance) = classify(&body) else {
            panic!("positive stdout must be accepted");
        };
        let envelope = decode_gpu_envelope(&fixture_bytes("gpu-envelope.tlv")).expect("envelope");
        let appraisal = build_gpu_appraisal(&acceptance.claim, &envelope, Vec::new())
            .expect("accepted claim builds appraisal");

        assert_eq!(appraisal.driver_version, "595.71.05");
        assert_eq!(appraisal.vbios_version, "96.00.88.00.11");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
        assert_eq!(
            appraisal.envelope_gpu_uuid,
            "GPU-256cc88f-e93b-9396-b581-274543ea3235"
        );
        assert_eq!(appraisal.arch, "HOPPER");
    }

    #[test]
    fn gpu_neg_a_classifies_nonce_mismatch() {
        let body = parse_nvattest_stdout(&stdout("negA")).expect("negative stdout parses");
        assert_eq!(
            rejection(&body).reason,
            GpuAppraisalReason::GpuNonceMismatch
        );
    }

    #[test]
    fn gpu_neg_b_classifies_nonce_mismatch() {
        let body = parse_nvattest_stdout(&stdout("negB")).expect("negative stdout parses");
        assert_eq!(
            rejection(&body).reason,
            GpuAppraisalReason::GpuNonceMismatch
        );
    }

    #[test]
    fn gpu_neg_c_classifies_nonce_mismatch() {
        let body = parse_nvattest_stdout(&stdout("negC")).expect("negative stdout parses");
        assert_eq!(
            rejection(&body).reason,
            GpuAppraisalReason::GpuNonceMismatch
        );
    }

    #[test]
    fn gpu_claims_rejects_secboot_false() {
        let mut body = positive_body();
        claim_mut(&mut body).insert("secboot".to_owned(), json!(false));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_secboot_string() {
        let mut body = positive_body();
        claim_mut(&mut body).insert("secboot".to_owned(), json!("true"));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_secboot_int() {
        let mut body = positive_body();
        claim_mut(&mut body).insert("secboot".to_owned(), json!(1));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_debug_enabled() {
        let mut body = positive_body();
        claim_mut(&mut body).insert("dbgstat".to_owned(), json!("enabled"));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_driver_rim_signature_false() {
        let mut body = positive_body();
        claim_mut(&mut body).insert(
            "x-nvidia-gpu-driver-rim-signature-verified".to_owned(),
            json!(false),
        );
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_measres_failure() {
        let mut body = positive_body();
        claim_mut(&mut body).insert("measres".to_owned(), json!("fail"));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_missing_claim_key() {
        let mut body = positive_body();
        claim_mut(&mut body).remove("x-nvidia-gpu-attestation-report-parsed");
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_ocsp_bad() {
        let mut body = positive_body();
        claim_mut(&mut body)
            .get_mut("x-nvidia-gpu-attestation-report-cert-chain")
            .expect("certificate chain")
            .as_object_mut()
            .expect("certificate chain object")
            .insert("x-nvidia-cert-ocsp-status".to_owned(), json!("revoked"));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_claims_not_a_list() {
        let mut body = positive_body();
        body.as_object_mut()
            .expect("body object")
            .insert("claims".to_owned(), json!({}));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_result_code_as_bool_false() {
        let mut body = positive_body();
        body.as_object_mut()
            .expect("body object")
            .insert("result_code".to_owned(), json!(false));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_claims_version_mismatch() {
        let mut body = positive_body();
        claim_mut(&mut body).insert("x-nvidia-gpu-claims-version".to_owned(), json!("4.0"));
        assert_appraisal_failed(&body);
    }

    #[test]
    fn gpu_claims_rejects_empty_stdout() {
        assert_eq!(parse_nvattest_stdout(""), Err(GpuClaimsError::StdoutEmpty));
    }

    #[test]
    fn gpu_claims_rejects_garbage_stdout() {
        assert_eq!(
            parse_nvattest_stdout("not json"),
            Err(GpuClaimsError::StdoutInvalidJson)
        );
    }

    #[test]
    fn gpu_claims_overall_eat_is_veto_only() {
        let mut detached_eat_object = positive_body();
        detached_eat_object
            .as_object_mut()
            .expect("body object")
            .insert("detached_eat".to_owned(), json!({}));
        assert_appraisal_failed(&detached_eat_object);

        let mut detached_eat_empty = positive_body();
        detached_eat_empty
            .as_object_mut()
            .expect("body object")
            .insert("detached_eat".to_owned(), json!([]));
        assert_appraisal_failed(&detached_eat_empty);

        let mut bad_algorithm = positive_body();
        mutate_overall_jwt(&mut bad_algorithm, Some(json!({"alg": "HS256"})), None);
        assert_appraisal_failed(&bad_algorithm);

        let mut bad_issuer = positive_body();
        mutate_overall_jwt(&mut bad_issuer, None, Some(json!({"iss": "OTHER"})));
        assert_appraisal_failed(&bad_issuer);

        let mut bad_claim = positive_body();
        claim_mut(&mut bad_claim).insert(
            "x-nvidia-gpu-vbios-rim-version-match".to_owned(),
            json!(false),
        );
        assert_appraisal_failed(&bad_claim);
    }
}
