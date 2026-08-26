// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local P-256 CA creation, loading, and client-certificate signing.

use std::fmt;

use hkdf::Hkdf;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequestParams,
    DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256, SerialNumber,
};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::Sha256;
use time::{Duration, OffsetDateTime};
use x509_parser::oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY};
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::FromDer;
use x509_parser::x509::SubjectPublicKeyInfo;

pub const CA_COMMON_NAME: &str = "solstone link CA";
pub const VALIDITY_DAYS: i64 = 365 * 10;
pub const VALIDITY_BACKDATE: Duration = Duration::minutes(5);

const JID_HKDF_SALT: &[u8] = b"solstone/journal/v1";
const JID_HKDF_INFO: &[u8] = b"solstone/jid/uuidv8/v1";

pub struct LocalCa {
    certificate: Certificate,
    key: KeyPair,
    certificate_pem: String,
    spki_der: Vec<u8>,
}

impl LocalCa {
    pub fn certificate_pem(&self) -> &str {
        &self.certificate_pem
    }

    pub fn private_key_pem(&self) -> String {
        self.key.serialize_pem()
    }

    pub fn spki_der(&self) -> &[u8] {
        &self.spki_der
    }

    pub fn public_key_spki_pem(&self) -> String {
        self.key.public_key_pem()
    }
}

pub struct IssuedClientCertificate {
    #[cfg(test)]
    certificate: Certificate,
    pem: String,
    cid: String,
}

/// A freshly minted server certificate signed by the committed local CA.
pub struct IssuedServerCertificate {
    certificate_der: CertificateDer<'static>,
    private_key: PrivateKeyDer<'static>,
}

impl IssuedServerCertificate {
    pub fn certificate_der(&self) -> CertificateDer<'static> {
        self.certificate_der.clone()
    }

    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        self.private_key.clone_key()
    }
}

impl IssuedClientCertificate {
    pub fn pem(&self) -> &str {
        &self.pem
    }

    pub fn cid(&self) -> &str {
        &self.cid
    }

    #[cfg(test)]
    fn certificate(&self) -> &Certificate {
        &self.certificate
    }
}

#[derive(Debug)]
pub enum CaError {
    Rcgen(rcgen::Error),
    InvalidCa(&'static str),
    InvalidSpki(&'static str),
    Randomness,
}

impl fmt::Display for CaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rcgen(error) => error.fmt(formatter),
            Self::InvalidCa(message) => write!(formatter, "invalid local CA: {message}"),
            Self::InvalidSpki(message) => write!(formatter, "invalid journal SPKI: {message}"),
            Self::Randomness => formatter.write_str("system CSPRNG unavailable"),
        }
    }
}

impl std::error::Error for CaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rcgen(error) => Some(error),
            Self::InvalidCa(_) | Self::InvalidSpki(_) | Self::Randomness => None,
        }
    }
}

impl From<rcgen::Error> for CaError {
    fn from(error: rcgen::Error) -> Self {
        Self::Rcgen(error)
    }
}

pub fn generate_ca() -> Result<LocalCa, CaError> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let params = ca_certificate_params(OffsetDateTime::now_utc())?;
    let certificate = params.self_signed(&key)?;
    let certificate_pem = certificate.pem();
    let spki_der = key.public_key_der();
    Ok(LocalCa {
        certificate,
        key,
        certificate_pem,
        spki_der,
    })
}

pub fn load_ca(certificate_pem: &str, private_key_pem: &str) -> Result<LocalCa, CaError> {
    let (_, pem) = parse_x509_pem(certificate_pem.as_bytes())
        .map_err(|_| CaError::InvalidCa("certificate PEM could not be parsed"))?;
    let (_, x509) = x509_parser::parse_x509_certificate(&pem.contents)
        .map_err(|_| CaError::InvalidCa("certificate DER could not be parsed"))?;
    validate_ca_certificate(&x509)?;

    let key = KeyPair::from_pem_and_sign_algo(private_key_pem, &PKCS_ECDSA_P256_SHA256)?;
    let spki_der = x509.tbs_certificate.subject_pki.raw.to_vec();
    if key.public_key_der() != spki_der {
        return Err(CaError::InvalidCa(
            "certificate and private key do not match",
        ));
    }

    let params = CertificateParams::from_ca_cert_pem(certificate_pem)?;
    let certificate = params.self_signed(&key)?;
    Ok(LocalCa {
        certificate,
        key,
        certificate_pem: certificate_pem.to_owned(),
        spki_der,
    })
}

pub fn sign_csr(
    ca: &LocalCa,
    csr_pem: &str,
    device_label: &str,
) -> Result<IssuedClientCertificate, CaError> {
    let mut csr = CertificateSigningRequestParams::from_pem(csr_pem)?;
    if csr.public_key.algorithm() != &PKCS_ECDSA_P256_SHA256 {
        return Err(CaError::InvalidCa("CSR public key must be ECDSA-P256"));
    }

    sanitize_client_certificate_params(&mut csr.params, device_label, OffsetDateTime::now_utc())?;
    let certificate = csr.signed_by(&ca.certificate, &ca.key)?;
    let pem = certificate.pem();
    let cid = format!(
        "sha256:{}",
        spl_core::ca::sha256_hex(certificate.der().as_ref())
    );
    Ok(IssuedClientCertificate {
        #[cfg(test)]
        certificate,
        pem,
        cid,
    })
}

/// Mint a short-lived TLS server leaf from the committed local CA.
pub fn issue_server_certificate(
    ca: &LocalCa,
    home_label: &str,
) -> Result<IssuedServerCertificate, CaError> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let now = OffsetDateTime::now_utc();
    let mut params = CertificateParams::default();
    params.distinguished_name = common_name(&format!("solstone link ({home_label})"));
    // No SAN: spl-transport's pinned verifier validates the CA fingerprint,
    // not a DNS name. `mtls_config` also performs no certificate-validity check.
    params.subject_alt_names.clear();
    params.is_ca = IsCa::ExplicitNoCa;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = now - VALIDITY_BACKDATE;
    // A new leaf is generated at each door start; it has a 30-day residual lifetime.
    params.not_after = now + Duration::days(30);
    params.serial_number = Some(random_serial_number()?);
    let certificate = params.signed_by(&key, &ca.certificate, &ca.key)?;
    Ok(IssuedServerCertificate {
        certificate_der: CertificateDer::from(certificate.der().to_vec()),
        private_key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    })
}

pub fn jid_from_spki(spki_der: &[u8]) -> Result<String, CaError> {
    let (remaining, spki) = SubjectPublicKeyInfo::from_der(spki_der)
        .map_err(|_| CaError::InvalidSpki("DER could not be parsed"))?;
    if !remaining.is_empty()
        || spki.algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY
        || spki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.as_oid().ok())
            != Some(OID_EC_P256)
    {
        return Err(CaError::InvalidSpki("requires an EC P-256 public key"));
    }

    // rcgen emits canonical P-256 SubjectPublicKeyInfo DER. This entry point is
    // used for locally generated CAs, and the cross-language vectors below prove
    // that their canonical representation agrees with Python's re-serialization.
    let hkdf = Hkdf::<Sha256>::new(Some(JID_HKDF_SALT), spki_der);
    let mut bytes = [0_u8; 16];
    hkdf.expand(JID_HKDF_INFO, &mut bytes)
        .map_err(|_| CaError::InvalidSpki("HKDF output length was rejected"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format_uuid_v8(bytes))
}

fn ca_certificate_params(now: OffsetDateTime) -> Result<CertificateParams, CaError> {
    let mut params = CertificateParams::default();
    params.distinguished_name = common_name(CA_COMMON_NAME);
    params.not_before = now - VALIDITY_BACKDATE;
    params.not_after = now + Duration::days(VALIDITY_DAYS);
    params.serial_number = Some(random_serial_number()?);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    Ok(params)
}

fn sanitize_client_certificate_params(
    params: &mut CertificateParams,
    device_label: &str,
    now: OffsetDateTime,
) -> Result<(), CaError> {
    params.distinguished_name = common_name(device_label);
    params.subject_alt_names.clear();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.is_ca = IsCa::ExplicitNoCa;
    params.not_before = now - VALIDITY_BACKDATE;
    params.not_after = now + Duration::days(VALIDITY_DAYS);
    params.serial_number = Some(random_serial_number()?);
    Ok(())
}

fn random_serial_number() -> Result<SerialNumber, CaError> {
    let random = SystemRandom::new();
    loop {
        let mut bytes = [0_u8; 20];
        random.fill(&mut bytes).map_err(|_| CaError::Randomness)?;
        bytes[0] &= 0x7f;
        if bytes.iter().any(|byte| *byte != 0) {
            return Ok(SerialNumber::from_slice(&bytes));
        }
    }
}

fn common_name(value: &str) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, value);
    name
}

fn format_uuid_v8(bytes: [u8; 16]) -> String {
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn validate_ca_certificate(
    certificate: &x509_parser::certificate::X509Certificate<'_>,
) -> Result<(), CaError> {
    let constraints = certificate
        .basic_constraints()
        .map_err(|_| CaError::InvalidCa("basic constraints could not be parsed"))?
        .ok_or(CaError::InvalidCa("basic constraints are missing"))?;
    if !constraints.critical
        || !constraints.value.ca
        || constraints.value.path_len_constraint != Some(0)
    {
        return Err(CaError::InvalidCa("must be a constrained CA"));
    }
    let usages = certificate
        .key_usage()
        .map_err(|_| CaError::InvalidCa("key usage could not be parsed"))?
        .ok_or(CaError::InvalidCa("key usage is missing"))?;
    if !usages.critical
        || !usages.value.digital_signature()
        || !usages.value.key_cert_sign()
        || !usages.value.crl_sign()
    {
        return Err(CaError::InvalidCa("key usage does not permit CA signing"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, DnType, KeyPair, SanType};

    use super::*;

    #[test]
    fn jid_vectors_match_python() {
        let cases = [
            (
                "3059301306072a8648ce3d020106082a8648ce3d03010703420004471c3e758c4904285bba7e53118ed0f524adeb0757d25bd2f8e7b0d76dfa714cdd520f7aca8a8b917acc37f51de8f0c9bbe3ad858382e702dc25a12d09f7a858",
                "f30ed159-ef46-8e9c-913f-e49f0fe7d201",
            ),
            (
                "3059301306072a8648ce3d020106082a8648ce3d030107034200047cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc4766997807775510db8ed040293d9ac69f7430dbba7dade63ce982299e04b79d227873d1",
                "62bde3af-1ef4-8292-84db-1e5ac2c07e8b",
            ),
            (
                "3059301306072a8648ce3d020106082a8648ce3d030107034200048e533b6fa0bf7b4625bb30667c01fb607ef9f8b8a80fef5b300628703187b2a373eb1dbde03318366d069f83a6f5900053c73633cb041b21c55e1a86c1f400b4",
                "75e46c0d-1c50-892b-98ff-4d174c135add",
            ),
            (
                "3059301306072a8648ce3d020106082a8648ce3d03010703420004ea68d7b6fedf0b71878938d51d71f8729e0acb8c2c6df8b3d79e8a4b90949ee02a2744c972c9fce787014a964a8ea0c84d714feaa4de823fe85a224a4dd048fa",
                "bb8f23b4-fd5e-8ca9-98c7-c1dfa927a840",
            ),
        ];

        for (spki, expected) in cases {
            assert_eq!(jid_from_spki(&decode_hex(spki)).unwrap(), expected);
        }
    }

    #[test]
    fn generated_ca_has_required_material() {
        let ca = generate_ca().unwrap();
        assert!(ca.certificate_pem().contains("BEGIN CERTIFICATE"));
        assert!(ca.private_key_pem().contains("BEGIN PRIVATE KEY"));
        assert!(jid_from_spki(ca.spki_der()).is_ok());
    }

    fn hostile_csr(params: CertificateParams) -> String {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        params.serialize_request(&key).unwrap().pem().unwrap()
    }

    #[test]
    fn hostile_csr_subject_is_replaced() {
        let ca = generate_ca().unwrap();
        let mut params = CertificateParams::default();
        params.distinguished_name = common_name("attacker subject");
        let csr_pem = hostile_csr(params);

        let parsed = CertificateSigningRequestParams::from_pem(&csr_pem).unwrap();
        assert_common_name(&parsed.params, "attacker subject");
        let issued = sign_csr(&ca, &csr_pem, "safe device").unwrap();
        assert_common_name(issued.certificate().params(), "safe device");
    }

    #[test]
    fn hostile_csr_sans_are_cleared() {
        let ca = generate_ca().unwrap();
        let mut params = CertificateParams::default();
        params.subject_alt_names = vec![SanType::DnsName("attacker.test".try_into().unwrap())];
        let csr_pem = hostile_csr(params);

        let parsed = CertificateSigningRequestParams::from_pem(&csr_pem).unwrap();
        assert_eq!(parsed.params.subject_alt_names.len(), 1);
        let issued = sign_csr(&ca, &csr_pem, "safe device").unwrap();
        assert!(issued.certificate().params().subject_alt_names.is_empty());
    }

    #[test]
    fn hostile_csr_key_usages_are_replaced() {
        let ca = generate_ca().unwrap();
        let mut params = CertificateParams::default();
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let csr_pem = hostile_csr(params);

        let parsed = CertificateSigningRequestParams::from_pem(&csr_pem).unwrap();
        assert_eq!(parsed.params.key_usages, vec![KeyUsagePurpose::KeyCertSign]);
        let issued = sign_csr(&ca, &csr_pem, "safe device").unwrap();
        assert_eq!(
            issued.certificate().params().key_usages,
            vec![KeyUsagePurpose::DigitalSignature]
        );
    }

    #[test]
    fn hostile_csr_extended_key_usages_are_replaced() {
        let ca = generate_ca().unwrap();
        let mut params = CertificateParams::default();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let csr_pem = hostile_csr(params);

        let parsed = CertificateSigningRequestParams::from_pem(&csr_pem).unwrap();
        assert_eq!(
            parsed.params.extended_key_usages,
            vec![ExtendedKeyUsagePurpose::ServerAuth]
        );
        let issued = sign_csr(&ca, &csr_pem, "safe device").unwrap();
        assert_eq!(
            issued.certificate().params().extended_key_usages,
            vec![ExtendedKeyUsagePurpose::ClientAuth]
        );
    }

    #[test]
    fn hostile_csr_ca_request_still_issues_non_ca_certificate() {
        let ca = generate_ca().unwrap();
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();

        // rcgen will not encode BasicConstraints into a CSR, and its parser
        // rejects that extension too. Keep both sides of that structural
        // barrier explicit, while also covering the certificate invariant.
        assert!(matches!(
            params.serialize_request(&key),
            Err(rcgen::Error::UnsupportedInCsr)
        ));

        let csr_pem = hostile_csr(CertificateParams::default());
        let issued = sign_csr(&ca, &csr_pem, "safe device").unwrap();
        assert_eq!(issued.certificate().params().is_ca, IsCa::ExplicitNoCa);
    }

    #[test]
    fn client_certificate_validity_is_overwritten() {
        let ca = generate_ca().unwrap();
        let mut params = CertificateParams::default();
        params.not_before = OffsetDateTime::UNIX_EPOCH;
        params.not_after = OffsetDateTime::UNIX_EPOCH + Duration::days(1);
        let csr_pem = hostile_csr(params);

        let before_signing = OffsetDateTime::now_utc();
        let issued = sign_csr(&ca, &csr_pem, "safe device").unwrap();
        let issued_params = issued.certificate().params();
        assert!(issued_params.not_before >= before_signing - Duration::minutes(6));
        assert!(issued_params.not_before <= before_signing - Duration::minutes(4));
        assert!(issued_params.not_after >= before_signing + Duration::days(3649));
        assert!(issued_params.not_after <= before_signing + Duration::days(3651));
    }

    #[test]
    fn signing_same_csr_mints_distinct_cids() {
        let ca = generate_ca().unwrap();
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let csr_pem = CertificateParams::default()
            .serialize_request(&key)
            .unwrap()
            .pem()
            .unwrap();

        let first = sign_csr(&ca, &csr_pem, "phone").unwrap();
        let second = sign_csr(&ca, &csr_pem, "phone").unwrap();

        assert_ne!(first.cid(), second.cid());
        assert!(first.cid().starts_with("sha256:"));
        assert_eq!(first.cid().len(), 71);
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|chunk| {
                let high = (chunk[0] as char).to_digit(16).unwrap();
                let low = (chunk[1] as char).to_digit(16).unwrap();
                ((high << 4) | low) as u8
            })
            .collect()
    }

    fn assert_common_name(params: &CertificateParams, expected: &str) {
        assert!(matches!(
            params.distinguished_name.get(&DnType::CommonName),
            Some(rcgen::DnValue::Utf8String(value)) if value == expected
        ));
    }
}
