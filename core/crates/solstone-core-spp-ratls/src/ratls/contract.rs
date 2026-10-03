// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use ring::digest::{SHA256, digest};

use crate::error::RatlsContractError;

pub const PREFACE_MAGIC: &[u8] = b"SPPRAT1\0";
pub const OWNER_NONCE_BYTES: usize = 32;
pub const EXPORTER_BYTES: usize = 32;
pub const COMPOSITE_EVIDENCE_OID: &str = "2.25.3708997813.3535365757.2172800616.1077671698";
pub const EXPORTER_LABEL: &[u8] = b"EXPERIMENTAL-sol-spp-engine-attestation-v1";
pub const EXPORTER_CONTEXT_DOMAIN: &[u8] = b"sol-spp-ratls-exporter-context-v1";
pub const CERTIFICATE_BINDING_DOMAIN: &[u8] = b"sol-spp-ratls-certificate-bind-v1";
pub const EXPORTER_BINDING_DOMAIN: &[u8] = b"sol-spp-ratls-exporter-bind-v1";
pub const EXPORTER_PROOF_PATH: &str = "/._sol/spp/exporter-proof";
pub const COMPOSITE_MEDIA_TYPE: &str = "application/vnd.sol.spp-composite-evidence-v1+der";
pub const EXPORTER_PROOF_MEDIA_TYPE: &str = "application/vnd.sol.spp-exporter-proof-v1+der";
pub const PROTOCOL_VERSION: u64 = 1;
pub const CERTIFICATE_FORMULA: &str =
    "SHA256(domain || nonce || SHA256(tls_spki_der) || SHA256(SPPGPU1_TLV))";
pub const EXPORTER_CONTEXT_FORMULA: &str =
    "SHA256(context_domain || nonce || SHA256(tls_spki_der))";
pub const EXPORTER_FORMULA: &str =
    "SHA256(domain || nonce || SHA256(tls_spki_der) || tls_exporter || SHA256(SPPGPU1_TLV))";

/// Separate, noncritical extension carrying raw NVIDIA OCSP responses for an
/// image whose locally selected profile requires offline signed-age status.
pub const STATUS_PROOFS_OID: &str = "2.25.2270882455.2325790823.606324700.3809542406";
pub const STATUS_PROOFS_OID_ARCS: [u64; 6] = [
    2,
    25,
    2_270_882_455,
    2_325_790_823,
    606_324_700,
    3_809_542_406,
];
pub const STATUS_PROOFS_VERSION: u64 = 1;
pub const STATUS_PROOFS_MEDIA_TYPE: &str = "application/vnd.sol.spp-status-proofs-v1+der";
pub const STATUS_PROOFS_ASN1: &str =
    "SEQUENCE { version INTEGER (1), responses SEQUENCE OF OCTET STRING }";
/// Bounds, fixed from measured real responses (891-968 bytes each; the
/// eight-entry H100 inventory bundles to 7,543 bytes) and the 65,535-byte
/// handshake-message limit of the journal's TLS client.
pub const MAX_STATUS_PROOFS_BYTES: usize = 16_384;
pub const MAX_STATUS_PROOF_RESPONSES: usize = 16;
pub const MAX_STATUS_PROOF_RESPONSE_BYTES: usize = 2_048;
/// The whole engine certificate is refused above this before it is parsed.
pub const MAX_CERTIFICATE_BYTES: usize = 49_152;

pub const COMPOSITE_FIELDS: [&str; 13] = [
    "version",
    "owner_nonce",
    "tls_spki_der",
    "amd_report",
    "hcl_report",
    "ak_public_key_pem",
    "quote_message",
    "quote_signature",
    "quote_pcrs",
    "amd_ark_pem",
    "amd_ask_pem",
    "amd_vcek_pem",
    "gpu_envelope",
];
pub const EXPORTER_PROOF_FIELDS: [&str; 7] = [
    "version",
    "owner_nonce",
    "tls_spki_der",
    "tls_exporter",
    "quote_message",
    "quote_signature",
    "quote_pcrs",
];

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend_from_slice(part);
    }
    digest(&SHA256, &bytes)
        .as_ref()
        .try_into()
        .expect("SHA-256 length")
}
pub fn certificate_binding(owner_nonce: &[u8], spki_der: &[u8], gpu_envelope: &[u8]) -> [u8; 32] {
    sha256(&[
        CERTIFICATE_BINDING_DOMAIN,
        owner_nonce,
        &sha256(&[spki_der]),
        &sha256(&[gpu_envelope]),
    ])
}
pub fn exporter_context(owner_nonce: &[u8], spki_der: &[u8]) -> [u8; 32] {
    sha256(&[EXPORTER_CONTEXT_DOMAIN, owner_nonce, &sha256(&[spki_der])])
}
pub fn exporter_binding(
    owner_nonce: &[u8],
    spki_der: &[u8],
    tls_exporter: &[u8],
    gpu_envelope: &[u8],
) -> [u8; 32] {
    sha256(&[
        EXPORTER_BINDING_DOMAIN,
        owner_nonce,
        &sha256(&[spki_der]),
        tls_exporter,
        &sha256(&[gpu_envelope]),
    ])
}

fn der_length(length: usize) -> Vec<u8> {
    if length < 128 {
        vec![length as u8]
    } else {
        let raw = length.to_be_bytes();
        let value = raw
            .iter()
            .skip_while(|byte| **byte == 0)
            .copied()
            .collect::<Vec<_>>();
        let mut out = vec![0x80 | value.len() as u8];
        out.extend(value);
        out
    }
}
fn der_tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(der_length(value.len()));
    out.extend(value);
    out
}
fn der_integer(value: u64) -> Vec<u8> {
    let raw = value.to_be_bytes();
    let mut bytes = raw
        .iter()
        .skip_while(|byte| **byte == 0)
        .copied()
        .collect::<Vec<_>>();
    if bytes.is_empty() {
        bytes.push(0);
    }
    if bytes[0] & 0x80 != 0 {
        bytes.insert(0, 0);
    }
    der_tlv(0x02, &bytes)
}

pub fn encode_sequence(version: u64, fields: &[&[u8]]) -> Vec<u8> {
    let mut body = der_integer(version);
    for field in fields {
        body.extend(der_tlv(0x04, field));
    }
    der_tlv(0x30, &body)
}
fn read_length(data: &[u8], offset: usize) -> Result<(usize, usize), RatlsContractError> {
    let first = *data
        .get(offset)
        .ok_or(RatlsContractError::TruncatedLength)?;
    if first < 128 {
        return Ok((usize::from(first), offset + 1));
    }
    let count = usize::from(first & 0x7f);
    if count == 0 || count > 4 || offset + 1 + count > data.len() {
        return Err(RatlsContractError::InvalidLength);
    }
    let bytes = &data[offset + 1..offset + 1 + count];
    if bytes[0] == 0 {
        return Err(RatlsContractError::NonMinimalLength);
    }
    let value = bytes
        .iter()
        .fold(0usize, |value, byte| (value << 8) | usize::from(*byte));
    if value < 128 {
        return Err(RatlsContractError::NonMinimalLength);
    }
    Ok((value, offset + 1 + count))
}
fn read_tlv(data: &[u8], offset: usize, tag: u8) -> Result<(&[u8], usize), RatlsContractError> {
    if data.get(offset) != Some(&tag) {
        return Err(RatlsContractError::UnexpectedTag);
    }
    let (length, start) = read_length(data, offset + 1)?;
    let end = start
        .checked_add(length)
        .ok_or(RatlsContractError::TruncatedValue)?;
    let value = data
        .get(start..end)
        .ok_or(RatlsContractError::TruncatedValue)?;
    Ok((value, end))
}
pub fn decode_sequence(
    data: &[u8],
    octet_count: usize,
) -> Result<(u64, Vec<Vec<u8>>), RatlsContractError> {
    let (body, end) = read_tlv(data, 0, 0x30)?;
    if end != data.len() {
        return Err(RatlsContractError::TrailingBytes);
    }
    let (integer, mut offset) = read_tlv(body, 0, 0x02)?;
    if integer.is_empty()
        || (integer.len() > 1 && integer[0] == 0 && integer[1] < 0x80)
        || integer.len() > 9
        || (integer.len() == 9 && integer[0] != 0)
    {
        return Err(RatlsContractError::NonMinimalInteger);
    }
    let version = integer
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    let mut fields = Vec::with_capacity(octet_count);
    for _ in 0..octet_count {
        let (field, next) = read_tlv(body, offset, 0x04)?;
        fields.push(field.to_vec());
        offset = next;
    }
    if offset != body.len() {
        return Err(RatlsContractError::UnexpectedField);
    }
    Ok((version, fields))
}

/// Encodes a version 1 status-proof extension value. Each response is one
/// complete, unchanged NVIDIA OCSPResponse DER.
pub fn encode_status_proofs(responses: &[&[u8]]) -> Vec<u8> {
    let mut list = Vec::new();
    for response in responses {
        list.extend(der_tlv(0x04, response));
    }
    let mut body = der_integer(STATUS_PROOFS_VERSION);
    body.extend(der_tlv(0x30, &list));
    der_tlv(0x30, &body)
}

/// Validates a version 1 status-proof extension value in place and returns
/// its response count. Every length and count is bounded before anything is
/// copied; non-minimal lengths, trailing bytes, an unknown version, an empty
/// bundle and oversized entries are refused. The responses themselves are
/// verified only by the pinned nvattest helper.
pub fn validate_status_proofs(data: &[u8]) -> Result<usize, RatlsContractError> {
    if data.len() > MAX_STATUS_PROOFS_BYTES {
        return Err(RatlsContractError::StatusProofsTooLarge);
    }
    let (body, end) = read_tlv(data, 0, 0x30)?;
    if end != data.len() {
        return Err(RatlsContractError::TrailingBytes);
    }
    let (version, offset) = read_tlv(body, 0, 0x02)?;
    if version != [STATUS_PROOFS_VERSION as u8] {
        return Err(RatlsContractError::UnsupportedVersion);
    }
    let (list, list_end) = read_tlv(body, offset, 0x30)?;
    if list_end != body.len() {
        return Err(RatlsContractError::UnexpectedField);
    }
    let mut count = 0usize;
    let mut cursor = 0usize;
    while cursor < list.len() {
        if count == MAX_STATUS_PROOF_RESPONSES {
            return Err(RatlsContractError::StatusProofsTooLarge);
        }
        let (response, next) = read_tlv(list, cursor, 0x04)?;
        if response.is_empty() || response.len() > MAX_STATUS_PROOF_RESPONSE_BYTES {
            return Err(RatlsContractError::StatusProofsTooLarge);
        }
        count += 1;
        cursor = next;
    }
    if count == 0 {
        return Err(RatlsContractError::StatusProofsEmpty);
    }
    Ok(count)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeEvidence {
    pub owner_nonce: Vec<u8>,
    pub tls_spki_der: Vec<u8>,
    pub amd_report: Vec<u8>,
    pub hcl_report: Vec<u8>,
    pub ak_public_key_pem: Vec<u8>,
    pub quote_message: Vec<u8>,
    pub quote_signature: Vec<u8>,
    pub quote_pcrs: Vec<u8>,
    pub amd_ark_pem: Vec<u8>,
    pub amd_ask_pem: Vec<u8>,
    pub amd_vcek_pem: Vec<u8>,
    pub gpu_envelope: Vec<u8>,
}
impl CompositeEvidence {
    pub fn to_der(&self) -> Vec<u8> {
        encode_sequence(
            PROTOCOL_VERSION,
            &[
                &self.owner_nonce,
                &self.tls_spki_der,
                &self.amd_report,
                &self.hcl_report,
                &self.ak_public_key_pem,
                &self.quote_message,
                &self.quote_signature,
                &self.quote_pcrs,
                &self.amd_ark_pem,
                &self.amd_ask_pem,
                &self.amd_vcek_pem,
                &self.gpu_envelope,
            ],
        )
    }
    pub fn from_der(data: &[u8]) -> Result<Self, RatlsContractError> {
        let (version, fields) = decode_sequence(data, 12)?;
        if version != PROTOCOL_VERSION {
            return Err(RatlsContractError::UnsupportedVersion);
        }
        Ok(Self {
            owner_nonce: fields[0].clone(),
            tls_spki_der: fields[1].clone(),
            amd_report: fields[2].clone(),
            hcl_report: fields[3].clone(),
            ak_public_key_pem: fields[4].clone(),
            quote_message: fields[5].clone(),
            quote_signature: fields[6].clone(),
            quote_pcrs: fields[7].clone(),
            amd_ark_pem: fields[8].clone(),
            amd_ask_pem: fields[9].clone(),
            amd_vcek_pem: fields[10].clone(),
            gpu_envelope: fields[11].clone(),
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExporterProof {
    pub owner_nonce: Vec<u8>,
    pub tls_spki_der: Vec<u8>,
    pub tls_exporter: Vec<u8>,
    pub quote_message: Vec<u8>,
    pub quote_signature: Vec<u8>,
    pub quote_pcrs: Vec<u8>,
}
impl ExporterProof {
    pub fn to_der(&self) -> Vec<u8> {
        encode_sequence(
            PROTOCOL_VERSION,
            &[
                &self.owner_nonce,
                &self.tls_spki_der,
                &self.tls_exporter,
                &self.quote_message,
                &self.quote_signature,
                &self.quote_pcrs,
            ],
        )
    }
    pub fn from_der(data: &[u8]) -> Result<Self, RatlsContractError> {
        let (version, fields) = decode_sequence(data, 6)?;
        if version != PROTOCOL_VERSION {
            return Err(RatlsContractError::UnsupportedVersion);
        }
        Ok(Self {
            owner_nonce: fields[0].clone(),
            tls_spki_der: fields[1].clone(),
            tls_exporter: fields[2].clone(),
            quote_message: fields[3].clone(),
            quote_signature: fields[4].clone(),
            quote_pcrs: fields[5].clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn string_array(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .expect("artifact field is an array")
            .iter()
            .map(|item| item.as_str().expect("artifact array item is a string"))
            .collect()
    }

    #[test]
    fn checked_in_contract_matches_structurally() {
        let artifact: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/ratls-contract.json"),
            )
            .expect("checked-in contract artifact is readable"),
        )
        .expect("valid checked-in contract artifact");
        assert_eq!(
            artifact["protocol_version"].as_u64(),
            Some(PROTOCOL_VERSION)
        );
        assert_eq!(
            artifact["preface"]["magic_ascii_nul"].as_str(),
            Some("SPPRAT1")
        );
        assert_eq!(PREFACE_MAGIC, b"SPPRAT1\0");
        assert_eq!(
            artifact["preface"]["owner_nonce_bytes"].as_u64(),
            Some(OWNER_NONCE_BYTES as u64)
        );
        assert_eq!(
            artifact["exporter"]["length"].as_u64(),
            Some(EXPORTER_BYTES as u64)
        );
        assert_eq!(
            artifact["x509_extension"]["oid"].as_str(),
            Some(COMPOSITE_EVIDENCE_OID)
        );
        assert_eq!(artifact["x509_extension"]["critical"].as_bool(), Some(true));
        assert_eq!(artifact["x509_extension"]["encoding"].as_str(), Some("DER"));
        assert_eq!(artifact["exporter"]["proof_encoding"].as_str(), Some("DER"));
        assert_eq!(
            artifact["x509_extension"]["media_type"].as_str(),
            Some(COMPOSITE_MEDIA_TYPE)
        );
        assert_eq!(
            artifact["exporter"]["proof_media_type"].as_str(),
            Some(EXPORTER_PROOF_MEDIA_TYPE)
        );
        assert_eq!(
            artifact["exporter"]["proof_path"].as_str(),
            Some(EXPORTER_PROOF_PATH)
        );
        assert_eq!(
            artifact["exporter"]["label"].as_str(),
            Some(std::str::from_utf8(EXPORTER_LABEL).expect("ASCII label"))
        );
        assert_eq!(
            artifact["exporter"]["context_domain"].as_str(),
            Some(std::str::from_utf8(EXPORTER_CONTEXT_DOMAIN).expect("ASCII domain"))
        );
        assert_eq!(
            artifact["binding"]["certificate_domain"].as_str(),
            Some(std::str::from_utf8(CERTIFICATE_BINDING_DOMAIN).expect("ASCII domain"))
        );
        assert_eq!(
            artifact["binding"]["exporter_domain"].as_str(),
            Some(std::str::from_utf8(EXPORTER_BINDING_DOMAIN).expect("ASCII domain"))
        );
        assert_eq!(
            artifact["binding"]["certificate_formula"].as_str(),
            Some(CERTIFICATE_FORMULA)
        );
        assert_eq!(
            artifact["binding"]["exporter_context_formula"].as_str(),
            Some(EXPORTER_CONTEXT_FORMULA)
        );
        assert_eq!(
            artifact["binding"]["exporter_formula"].as_str(),
            Some(EXPORTER_FORMULA)
        );
        assert_eq!(
            string_array(&artifact["x509_extension"]["fields"]),
            COMPOSITE_FIELDS
        );
        assert_eq!(
            string_array(&artifact["exporter"]["proof_fields"]),
            EXPORTER_PROOF_FIELDS
        );
    }
    fn repository_file(relative: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative)
    }

    fn decode_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex"))
            .collect()
    }

    #[test]
    fn checked_in_status_proof_contract_matches_the_code() {
        let artifact: Value = serde_json::from_str(
            &std::fs::read_to_string(repository_file("fixtures/ratls-contract.json"))
                .expect("contract artifact"),
        )
        .expect("contract JSON");
        let proofs = &artifact["status_proofs"];
        assert_eq!(proofs["oid"].as_str(), Some(STATUS_PROOFS_OID));
        assert_eq!(
            STATUS_PROOFS_OID,
            STATUS_PROOFS_OID_ARCS
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(".")
        );
        assert_ne!(STATUS_PROOFS_OID, COMPOSITE_EVIDENCE_OID);
        assert_eq!(proofs["critical"].as_bool(), Some(false));
        assert_eq!(proofs["encoding"].as_str(), Some("DER"));
        assert_eq!(proofs["version"].as_u64(), Some(STATUS_PROOFS_VERSION));
        assert_eq!(
            proofs["media_type"].as_str(),
            Some(STATUS_PROOFS_MEDIA_TYPE)
        );
        assert_eq!(proofs["asn1"].as_str(), Some(STATUS_PROOFS_ASN1));
        let limits = &proofs["limits"];
        assert_eq!(
            limits["max_extension_value_bytes"].as_u64(),
            Some(MAX_STATUS_PROOFS_BYTES as u64)
        );
        assert_eq!(
            limits["max_responses"].as_u64(),
            Some(MAX_STATUS_PROOF_RESPONSES as u64)
        );
        assert_eq!(
            limits["max_response_bytes"].as_u64(),
            Some(MAX_STATUS_PROOF_RESPONSE_BYTES as u64)
        );
        assert_eq!(
            limits["max_certificate_bytes"].as_u64(),
            Some(MAX_CERTIFICATE_BYTES as u64)
        );
        assert_eq!(
            proofs["verification"]["admission_min_remaining_seconds"].as_u64(),
            Some(crate::ratls::channel::OFFLINE_STATUS_MIN_REMAINING.as_secs())
        );
        // The bounds leave room for the evidence the engine already sends,
        // under the client's 65,535-byte handshake-message limit.
        let measured = &proofs["measured"];
        let largest_fixture = measured["fixture_certificate_bytes_without_proofs"]
            .as_u64()
            .expect("measured certificate") as usize;
        assert!(largest_fixture + MAX_STATUS_PROOFS_BYTES + 64 <= MAX_CERTIFICATE_BYTES);
        const { assert!(MAX_CERTIFICATE_BYTES + 1024 <= 65_535) };
        assert!(
            measured["h100_inventory_bundle_bytes"]
                .as_u64()
                .expect("bundle") as usize
                <= MAX_STATUS_PROOFS_BYTES
        );
        assert_eq!(
            proofs["vectors"].as_str(),
            Some("core/fixtures/ratls-status-proofs-v1-vectors.json")
        );
    }

    #[test]
    fn independent_status_proof_vectors_decode_as_published() {
        let vectors: Value = serde_json::from_str(
            &std::fs::read_to_string(repository_file(
                "fixtures/ratls-status-proofs-v1-vectors.json",
            ))
            .expect("vectors"),
        )
        .expect("vectors JSON");
        let vectors = vectors["vectors"].as_array().expect("vector list");
        assert!(vectors.len() >= 19);
        for vector in vectors {
            let name = vector["name"].as_str().expect("name");
            let bytes = decode_hex(vector["hex"].as_str().expect("hex"));
            assert_eq!(
                bytes.len() as u64,
                vector["bytes"].as_u64().unwrap(),
                "{name}"
            );
            let decoded = validate_status_proofs(&bytes);
            if vector["valid"].as_bool() == Some(true) {
                assert_eq!(
                    decoded,
                    Ok(vector["response_count"].as_u64().unwrap() as usize),
                    "{name}"
                );
            } else {
                assert!(decoded.is_err(), "{name} must be refused");
            }
        }
    }

    #[test]
    fn the_encoder_reproduces_the_independent_vectors() {
        let vectors: Value = serde_json::from_str(
            &std::fs::read_to_string(repository_file(
                "fixtures/ratls-status-proofs-v1-vectors.json",
            ))
            .expect("vectors"),
        )
        .expect("vectors JSON");
        let find = |name: &str| {
            vectors["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|vector| vector["name"] == name)
                .map(|vector| decode_hex(vector["hex"].as_str().unwrap()))
                .unwrap()
        };
        let small = [0x30u8, 0x03, 0x02, 0x01, 0x01];
        assert_eq!(encode_status_proofs(&[&small]), find("one_response"));
        assert_eq!(
            encode_status_proofs(&[&small[..]; MAX_STATUS_PROOF_RESPONSES]),
            find("max_responses")
        );
        let mut long = vec![0u8; MAX_STATUS_PROOF_RESPONSE_BYTES];
        long[0] = 0x30;
        assert_eq!(encode_status_proofs(&[&long]), find("max_response_bytes"));
    }

    #[test]
    fn evidence_round_trips() {
        let item = CompositeEvidence {
            owner_nonce: vec![1],
            tls_spki_der: vec![2],
            amd_report: vec![3],
            hcl_report: vec![4],
            ak_public_key_pem: vec![5],
            quote_message: vec![6],
            quote_signature: vec![7],
            quote_pcrs: vec![8],
            amd_ark_pem: vec![9],
            amd_ask_pem: vec![10],
            amd_vcek_pem: vec![11],
            gpu_envelope: vec![12],
        };
        assert_eq!(CompositeEvidence::from_der(&item.to_der()), Ok(item));
        let proof = ExporterProof {
            owner_nonce: vec![1],
            tls_spki_der: vec![2],
            tls_exporter: vec![3],
            quote_message: vec![4],
            quote_signature: vec![5],
            quote_pcrs: vec![6],
        };
        assert_eq!(ExporterProof::from_der(&proof.to_der()), Ok(proof));
    }
}
