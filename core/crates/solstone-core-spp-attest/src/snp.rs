// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! HCLA and SEV-SNP report parsing with pinned AMD chain verification.

use ring::{
    digest::{SHA256, digest},
    signature::{
        ECDSA_P384_SHA384_FIXED, RSA_PSS_2048_8192_SHA384, RsaPublicKeyComponents,
        UnparsedPublicKey,
    },
};
use serde_json::{Map, Value};
use x509_parser::{
    pem::parse_x509_pem,
    prelude::{FromDer, X509Certificate, X509Name},
    public_key::PublicKey,
    signature_algorithm::SignatureAlgorithm,
};

use crate::error::{SnpParseError, SnpVerifyError};

const HCL_SIG: &[u8; 4] = b"HCLA";
const HCL_REPORT_OFFSET: usize = 32;
const HCL_REPORT_SIZE: usize = 1184;
const HCL_RUNTIME_OFFSET: usize = HCL_REPORT_OFFSET + HCL_REPORT_SIZE;

const SNP_OFF_VERSION: usize = 0x000;
const SNP_OFF_GUEST_SVN: usize = 0x004;
const SNP_OFF_POLICY: usize = 0x008;
const SNP_OFF_VMPL: usize = 0x030;
const SNP_OFF_SIG_ALGO: usize = 0x034;
const SNP_OFF_CURRENT_TCB: usize = 0x038;
const SNP_OFF_PLATFORM_INFO: usize = 0x040;
const SNP_OFF_KEY_INFO: usize = 0x048;
const SNP_OFF_REPORT_DATA: usize = 0x050;
const SNP_OFF_MEASUREMENT: usize = 0x090;
const SNP_OFF_HOST_DATA: usize = 0x0c0;
const SNP_OFF_REPORTED_TCB: usize = 0x180;
const SNP_OFF_CPUID_FAMILY: usize = 0x188;
const SNP_OFF_CPUID_MODEL: usize = 0x189;
const SNP_OFF_CPUID_STEP: usize = 0x18a;
const SNP_OFF_CHIP_ID: usize = 0x1a0;
const SNP_OFF_COMMITTED_TCB: usize = 0x1e0;
const SNP_OFF_CURRENT_VERSION: usize = 0x1e8;
const SNP_OFF_COMMITTED_VERSION: usize = 0x1ec;
const SNP_OFF_LAUNCH_TCB: usize = 0x1f0;
const SNP_OFF_SIGNATURE: usize = 0x2a0;
const SNP_SIGNED_PREFIX_LEN: usize = 0x2a0;

const SHA384_OID: &str = "2.16.840.1.101.3.4.2.2";
const MGF1_OID: &str = "1.2.840.113549.1.1.8";
const PSS_SALT_LENGTH: u32 = 48;

/// CPU generation used solely to select the SNP TCB byte layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    PreTurin,
    Turin,
}

/// The fields of one SNP TCB version record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcbVersion {
    pub boot_loader: Option<u8>,
    pub tee: Option<u8>,
    pub snp: Option<u8>,
    pub microcode: Option<u8>,
    pub fmc: Option<u8>,
}

impl TcbVersion {
    fn from_raw(raw: [u8; 8], generation: Generation) -> Self {
        match generation {
            Generation::Turin => Self {
                fmc: Some(raw[0]),
                boot_loader: Some(raw[1]),
                tee: Some(raw[2]),
                snp: Some(raw[3]),
                microcode: Some(raw[7]),
            },
            Generation::PreTurin => Self {
                boot_loader: Some(raw[0]),
                tee: Some(raw[1]),
                snp: Some(raw[6]),
                microcode: Some(raw[7]),
                fmc: None,
            },
        }
    }
}

/// A parsed, fixed-size AMD SEV-SNP report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnpReport {
    pub raw: Vec<u8>,
    pub version: u32,
    pub guest_svn: u32,
    pub policy: u64,
    pub vmpl: u32,
    pub sig_algo: u32,
    pub platform_info: u64,
    pub key_info: u32,
    pub report_data: [u8; 64],
    pub measurement: [u8; 48],
    pub host_data: [u8; 32],
    pub chip_id: [u8; 64],
    pub cpuid_family: Option<u8>,
    pub cpuid_model: Option<u8>,
    pub cpuid_step: Option<u8>,
    pub generation: Generation,
    pub current_tcb: TcbVersion,
    pub reported_tcb: TcbVersion,
    pub committed_tcb: TcbVersion,
    pub launch_tcb: TcbVersion,
    pub current_version: String,
    pub committed_version: String,
}

impl SnpReport {
    /// Parses a complete 1184-byte SNP report.
    pub fn parse(raw: &[u8]) -> Result<Self, SnpParseError> {
        if raw.len() != HCL_REPORT_SIZE {
            return Err(SnpParseError::ReportLength);
        }
        let version = u32_at(raw, SNP_OFF_VERSION)?;
        let cpuid_family = (version >= 3)
            .then(|| byte_at(raw, SNP_OFF_CPUID_FAMILY))
            .transpose()?;
        let cpuid_model = (version >= 3)
            .then(|| byte_at(raw, SNP_OFF_CPUID_MODEL))
            .transpose()?;
        let cpuid_step = (version >= 3)
            .then(|| byte_at(raw, SNP_OFF_CPUID_STEP))
            .transpose()?;
        let generation = generation_for_cpuid(cpuid_family, cpuid_model);

        Ok(Self {
            raw: raw.to_vec(),
            version,
            guest_svn: u32_at(raw, SNP_OFF_GUEST_SVN)?,
            policy: u64_at(raw, SNP_OFF_POLICY)?,
            vmpl: u32_at(raw, SNP_OFF_VMPL)?,
            sig_algo: u32_at(raw, SNP_OFF_SIG_ALGO)?,
            platform_info: u64_at(raw, SNP_OFF_PLATFORM_INFO)?,
            key_info: u32_at(raw, SNP_OFF_KEY_INFO)?,
            report_data: array_at(raw, SNP_OFF_REPORT_DATA)?,
            measurement: array_at(raw, SNP_OFF_MEASUREMENT)?,
            host_data: array_at(raw, SNP_OFF_HOST_DATA)?,
            chip_id: array_at(raw, SNP_OFF_CHIP_ID)?,
            cpuid_family,
            cpuid_model,
            cpuid_step,
            generation,
            current_tcb: TcbVersion::from_raw(array_at(raw, SNP_OFF_CURRENT_TCB)?, generation),
            reported_tcb: TcbVersion::from_raw(array_at(raw, SNP_OFF_REPORTED_TCB)?, generation),
            committed_tcb: TcbVersion::from_raw(array_at(raw, SNP_OFF_COMMITTED_TCB)?, generation),
            launch_tcb: TcbVersion::from_raw(array_at(raw, SNP_OFF_LAUNCH_TCB)?, generation),
            current_version: version_at(array_at(raw, SNP_OFF_CURRENT_VERSION)?),
            committed_version: version_at(array_at(raw, SNP_OFF_COMMITTED_VERSION)?),
        })
    }
}

/// Returns the SNP TCB layout generation for a CPUID pair.
pub fn generation_for_cpuid(family: Option<u8>, model: Option<u8>) -> Generation {
    if family == Some(0x1a)
        && model
            .is_some_and(|model| (0x90..=0xaf).contains(&model) || (0xc0..=0xcf).contains(&model))
    {
        Generation::Turin
    } else {
        Generation::PreTurin
    }
}

/// The HCLA wrapper around an SNP report and its runtime JSON claim bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct HclaBlob {
    pub version: u32,
    pub request_type: u32,
    pub report: Vec<u8>,
    pub runtime_json: Vec<u8>,
    pub runtime: Map<String, Value>,
}

/// Parses an HCLA wrapper without applying HCLA-version policy.
pub fn parse_hcla(blob: &[u8]) -> Result<HclaBlob, SnpParseError> {
    if blob.len() < HCL_RUNTIME_OFFSET {
        return Err(SnpParseError::HclaTooShort);
    }
    if blob.get(..HCL_SIG.len()) != Some(HCL_SIG.as_slice()) {
        return Err(SnpParseError::HclaMagicMismatch);
    }
    let version = u32_at(blob, 4)?;
    let request_type = u32_at(blob, 12)?;
    if request_type != 2 {
        return Err(SnpParseError::HclaRequestTypeMismatch);
    }
    let report = blob
        .get(HCL_REPORT_OFFSET..HCL_RUNTIME_OFFSET)
        .ok_or(SnpParseError::HclaTooShort)?
        .to_vec();
    let runtime_area = blob
        .get(HCL_RUNTIME_OFFSET..)
        .ok_or(SnpParseError::HclaTooShort)?;
    let relative_start = runtime_area
        .windows(2)
        .position(|window| window == b"{\"")
        .ok_or(SnpParseError::RuntimeJsonNotFound)?;
    let runtime_start = HCL_RUNTIME_OFFSET + relative_start;
    let runtime_tail = blob
        .get(runtime_start..)
        .ok_or(SnpParseError::RuntimeJsonNotFound)?;
    let runtime_end = runtime_start
        + runtime_tail
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(runtime_tail.len());
    let runtime_json = blob
        .get(runtime_start..runtime_end)
        .ok_or(SnpParseError::RuntimeJsonNotFound)?
        .to_vec();
    let runtime = serde_json::from_slice::<Value>(&runtime_json)
        .map_err(|_| SnpParseError::RuntimeJsonInvalid)?;
    let Value::Object(runtime) = runtime else {
        return Err(SnpParseError::RuntimeJsonNotObject);
    };
    Ok(HclaBlob {
        version,
        request_type,
        report,
        runtime_json,
        runtime,
    })
}

/// Successful AMD chain and report-signature verification details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmdReportVerification {
    pub vcek_issuer_cn: String,
}

/// Verifies the selected pinned AMD chain and its VCEK report signature.
pub fn verify_amd_chain_and_report(
    report: &SnpReport,
    bundle_cert_pems: &[&[u8]],
    now_unix_seconds: i64,
) -> Result<AmdReportVerification, SnpVerifyError> {
    if bundle_cert_pems.is_empty() {
        return Err(SnpVerifyError::NoBundleCertificates);
    }
    let certs: Vec<ParsedCertificate> = bundle_cert_pems
        .iter()
        .map(|pem| parse_pem_certificate(pem))
        .collect::<Result<_, _>>()?;
    let vcek = select_vcek(&certs)?;
    let root_pair = select_root_generation(&vcek.issuer_cn, embedded_root_pairs())?;
    let ark = parse_pem_certificate(root_pair.ark_pem.as_bytes())?;
    let ask = parse_pem_certificate(root_pair.ask_pem.as_bytes())?;
    if !ark.is_ca || !ask.is_ca {
        return Err(SnpVerifyError::RootMaterialInvalid);
    }

    verify_rsa_pss_sha384_signature(&ask.der, &ark.der)?;
    verify_rsa_pss_sha384_signature(&ark.der, &ark.der)?;
    verify_rsa_pss_sha384_signature(&vcek.der, &ask.der)?;
    for certificate in [&ark, &ask, vcek] {
        if now_unix_seconds < certificate.not_before || now_unix_seconds > certificate.not_after {
            return Err(SnpVerifyError::CertificateTimeInvalid);
        }
    }
    reject_mismatched_bundle_cas(&certs, &ark, &ask)?;
    verify_report_signature(report, vcek)?;
    Ok(AmdReportVerification {
        vcek_issuer_cn: vcek.issuer_cn.clone(),
    })
}

/// Verifies an RSA-PSS-SHA384 certificate signature against an independent issuer certificate.
pub(crate) fn verify_rsa_pss_sha384_signature(
    certificate_der: &[u8],
    issuer_certificate_der: &[u8],
) -> Result<(), SnpVerifyError> {
    let certificate = parse_der_certificate(certificate_der)?;
    let issuer = parse_der_certificate(issuer_certificate_der)?;
    verify_certificate_signature(&certificate, &issuer)
}

pub(crate) struct EmbeddedRootPair {
    pub(crate) product: &'static str,
    ark_pem: &'static str,
    ask_pem: &'static str,
}

const EMBEDDED_ROOT_PAIRS: [EmbeddedRootPair; 3] = [
    EmbeddedRootPair {
        product: "Milan",
        ark_pem: include_str!("../roots/amd/Milan/ark.pem"),
        ask_pem: include_str!("../roots/amd/Milan/ask.pem"),
    },
    EmbeddedRootPair {
        product: "Genoa",
        ark_pem: include_str!("../roots/amd/Genoa/ark.pem"),
        ask_pem: include_str!("../roots/amd/Genoa/ask.pem"),
    },
    EmbeddedRootPair {
        product: "Turin",
        ark_pem: include_str!("../roots/amd/Turin/ark.pem"),
        ask_pem: include_str!("../roots/amd/Turin/ask.pem"),
    },
];

pub(crate) fn embedded_root_pairs() -> &'static [EmbeddedRootPair; 3] {
    &EMBEDDED_ROOT_PAIRS
}

pub(crate) fn select_root_generation<'a>(
    vcek_issuer_cn: &str,
    roots: &'a [EmbeddedRootPair],
) -> Result<&'a EmbeddedRootPair, SnpVerifyError> {
    for root in roots {
        if root.product.is_empty() {
            return Err(SnpVerifyError::RootMaterialInvalid);
        }
        let ask = parse_pem_certificate(root.ask_pem.as_bytes())?;
        if ask.subject_cn == vcek_issuer_cn {
            return Ok(root);
        }
    }
    Err(SnpVerifyError::UnknownRootGeneration)
}

enum PublicKeyMaterial {
    Rsa { modulus: Vec<u8>, exponent: Vec<u8> },
    Ec(Vec<u8>),
    Other,
}

struct ParsedCertificate {
    der: Vec<u8>,
    subject_cn: String,
    issuer_cn: String,
    tbs: Vec<u8>,
    signature: Vec<u8>,
    public_key: PublicKeyMaterial,
    is_ca: bool,
    not_before: i64,
    not_after: i64,
    uses_fixed_pss_sha384: bool,
}

fn parse_pem_certificate(pem_bytes: &[u8]) -> Result<ParsedCertificate, SnpVerifyError> {
    let (remaining, pem) =
        parse_x509_pem(pem_bytes).map_err(|_| SnpVerifyError::CertificateParse)?;
    if !remaining.is_empty() || pem.label != "CERTIFICATE" {
        return Err(SnpVerifyError::CertificateParse);
    }
    parse_der_certificate(&pem.contents)
}

fn parse_der_certificate(der: &[u8]) -> Result<ParsedCertificate, SnpVerifyError> {
    let (remaining, certificate) =
        X509Certificate::from_der(der).map_err(|_| SnpVerifyError::CertificateParse)?;
    if !remaining.is_empty() || certificate.signature_value.unused_bits != 0 {
        return Err(SnpVerifyError::CertificateParse);
    }
    let public_key = match certificate
        .public_key()
        .parsed()
        .map_err(|_| SnpVerifyError::CertificateParse)?
    {
        PublicKey::RSA(key) => PublicKeyMaterial::Rsa {
            modulus: without_leading_zeros(key.modulus),
            exponent: without_leading_zeros(key.exponent),
        },
        PublicKey::EC(point) => PublicKeyMaterial::Ec(point.data().to_vec()),
        _ => PublicKeyMaterial::Other,
    };
    let is_ca = certificate
        .basic_constraints()
        .map_err(|_| SnpVerifyError::CertificateParse)?
        .is_some_and(|extension| extension.value.ca);
    Ok(ParsedCertificate {
        der: der.to_vec(),
        subject_cn: common_name(certificate.subject())?,
        issuer_cn: common_name(certificate.issuer())?,
        tbs: certificate.tbs_certificate.as_ref().to_vec(),
        signature: certificate.signature_value.data.to_vec(),
        public_key,
        is_ca,
        not_before: certificate.validity().not_before.timestamp(),
        not_after: certificate.validity().not_after.timestamp(),
        uses_fixed_pss_sha384: fixed_pss_sha384(&certificate.signature_algorithm)
            && fixed_pss_sha384(&certificate.tbs_certificate.signature),
    })
}

fn common_name(name: &X509Name<'_>) -> Result<String, SnpVerifyError> {
    name.iter_common_name()
        .next()
        .and_then(|attribute| attribute.as_str().ok())
        .map(str::to_owned)
        .ok_or(SnpVerifyError::CertificateParse)
}

fn fixed_pss_sha384(algorithm: &x509_parser::x509::AlgorithmIdentifier<'_>) -> bool {
    let Ok(SignatureAlgorithm::RSASSA_PSS(parameters)) = SignatureAlgorithm::try_from(algorithm)
    else {
        return false;
    };
    let Ok(mask_generation) = parameters.mask_gen_algorithm() else {
        return false;
    };
    parameters.hash_algorithm_oid().to_id_string() == SHA384_OID
        && mask_generation.mgf.to_id_string() == MGF1_OID
        && mask_generation.hash.to_id_string() == SHA384_OID
        && parameters.salt_length() == PSS_SALT_LENGTH
        && parameters.trailer_field() == 1
}

fn select_vcek(certs: &[ParsedCertificate]) -> Result<&ParsedCertificate, SnpVerifyError> {
    let mut candidates = certs.iter().filter(|certificate| {
        !certificate.is_ca && matches!(certificate.public_key, PublicKeyMaterial::Ec(_))
    });
    let vcek = candidates
        .next()
        .ok_or(SnpVerifyError::VcekSelectionFailure)?;
    if candidates.next().is_some() {
        return Err(SnpVerifyError::VcekSelectionFailure);
    }
    Ok(vcek)
}

fn verify_certificate_signature(
    certificate: &ParsedCertificate,
    issuer: &ParsedCertificate,
) -> Result<(), SnpVerifyError> {
    if !certificate.uses_fixed_pss_sha384 {
        return Err(SnpVerifyError::UnsupportedCertificateAlgorithm);
    }
    let PublicKeyMaterial::Rsa { modulus, exponent } = &issuer.public_key else {
        return Err(SnpVerifyError::UnsupportedCertificateAlgorithm);
    };
    let key = RsaPublicKeyComponents {
        n: modulus,
        e: exponent,
    };
    key.verify(
        &RSA_PSS_2048_8192_SHA384,
        &certificate.tbs,
        &certificate.signature,
    )
    .map_err(|_| SnpVerifyError::ChainSignatureInvalid)
}

fn reject_mismatched_bundle_cas(
    certs: &[ParsedCertificate],
    ark: &ParsedCertificate,
    ask: &ParsedCertificate,
) -> Result<(), SnpVerifyError> {
    let ark_fingerprint = digest(&SHA256, &ark.der);
    let ask_fingerprint = digest(&SHA256, &ask.der);
    for certificate in certs {
        let expected = if certificate.subject_cn == ark.subject_cn {
            Some(ark_fingerprint.as_ref())
        } else if certificate.subject_cn == ask.subject_cn {
            Some(ask_fingerprint.as_ref())
        } else {
            None
        };
        if expected.is_some_and(|expected| digest(&SHA256, &certificate.der).as_ref() != expected) {
            return Err(SnpVerifyError::BundleCaMismatch);
        }
    }
    Ok(())
}

fn verify_report_signature(
    report: &SnpReport,
    vcek: &ParsedCertificate,
) -> Result<(), SnpVerifyError> {
    if report.raw.len() != HCL_REPORT_SIZE {
        return Err(SnpVerifyError::ReportSignatureInvalid);
    }
    let raw_signature = report
        .raw
        .get(SNP_OFF_SIGNATURE..HCL_REPORT_SIZE)
        .ok_or(SnpVerifyError::ReportSignatureInvalid)?;
    if raw_signature
        .get(144..)
        .ok_or(SnpVerifyError::ReportSignatureInvalid)?
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(SnpVerifyError::ReportSignatureReservedNonZero);
    }
    let r = fixed_signature_component(
        raw_signature
            .get(..72)
            .ok_or(SnpVerifyError::ReportSignatureInvalid)?,
    )?;
    let s = fixed_signature_component(
        raw_signature
            .get(72..144)
            .ok_or(SnpVerifyError::ReportSignatureInvalid)?,
    )?;
    let PublicKeyMaterial::Ec(public_key) = &vcek.public_key else {
        return Err(SnpVerifyError::UnsupportedCertificateAlgorithm);
    };
    let mut signature = [0u8; 96];
    signature[..48].copy_from_slice(&r);
    signature[48..].copy_from_slice(&s);
    UnparsedPublicKey::new(&ECDSA_P384_SHA384_FIXED, public_key)
        .verify(
            report
                .raw
                .get(..SNP_SIGNED_PREFIX_LEN)
                .ok_or(SnpVerifyError::ReportSignatureInvalid)?,
            &signature,
        )
        .map_err(|_| SnpVerifyError::ReportSignatureInvalid)
}

fn fixed_signature_component(raw: &[u8]) -> Result<[u8; 48], SnpVerifyError> {
    let raw: [u8; 72] = raw
        .try_into()
        .map_err(|_| SnpVerifyError::ReportSignatureInvalid)?;
    if raw[48..].iter().any(|byte| *byte != 0) {
        return Err(SnpVerifyError::ReportSignatureScalarOverflow);
    }
    let mut component = [0u8; 48];
    for (index, byte) in raw[..48].iter().rev().enumerate() {
        component[index] = *byte;
    }
    Ok(component)
}

fn without_leading_zeros(value: &[u8]) -> Vec<u8> {
    value
        .iter()
        .skip_while(|byte| **byte == 0)
        .copied()
        .collect()
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, SnpParseError> {
    Ok(u32::from_le_bytes(array_at(data, offset)?))
}

fn u64_at(data: &[u8], offset: usize) -> Result<u64, SnpParseError> {
    Ok(u64::from_le_bytes(array_at(data, offset)?))
}

fn byte_at(data: &[u8], offset: usize) -> Result<u8, SnpParseError> {
    data.get(offset).copied().ok_or(SnpParseError::ReportLength)
}

fn array_at<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N], SnpParseError> {
    let end = offset.checked_add(N).ok_or(SnpParseError::ReportLength)?;
    data.get(offset..end)
        .ok_or(SnpParseError::ReportLength)?
        .try_into()
        .map_err(|_| SnpParseError::ReportLength)
}

fn version_at(raw: [u8; 3]) -> String {
    format!("{}.{}.{}", raw[2], raw[1], raw[0])
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{
        HCL_REPORT_OFFSET, SNP_OFF_MEASUREMENT, SnpParseError, SnpReport, SnpVerifyError,
        embedded_root_pairs, parse_hcla, select_root_generation, verify_amd_chain_and_report,
        verify_rsa_pss_sha384_signature,
    };
    use crate::test_support::fixture_bytes;

    const VALID_NOW_UNIX_SECONDS: i64 = 1_800_000_000;

    fn fixture_certificates() -> [Vec<u8>; 3] {
        [
            fixture_bytes("certs/ark.pem"),
            fixture_bytes("certs/ask.pem"),
            fixture_bytes("certs/vcek.pem"),
        ]
    }

    fn fixture_report() -> SnpReport {
        SnpReport::parse(&fixture_bytes("report.bin")).expect("fixture report parses")
    }

    fn assert_fixture_tcb(report: &SnpReport) {
        for tcb in [
            &report.current_tcb,
            &report.reported_tcb,
            &report.committed_tcb,
            &report.launch_tcb,
        ] {
            assert_eq!(tcb.boot_loader, Some(10));
            assert_eq!(tcb.tee, Some(0));
            assert_eq!(tcb.snp, Some(27));
            assert_eq!(tcb.microcode, Some(88));
            assert_eq!(tcb.fmc, None);
        }
    }

    #[test]
    fn snp_report_fixture_parses() {
        let report = fixture_report();
        assert_eq!(report.version, 5);
        assert_eq!(report.cpuid_family, Some(25));
        assert_eq!(report.cpuid_model, Some(17));
        assert_eq!(report.cpuid_step, Some(1));
        assert_fixture_tcb(&report);
    }

    #[test]
    fn hcla_report_matches_standalone_report() {
        let hcla = parse_hcla(&fixture_bytes("hcl_report.bin")).expect("fixture HCLA parses");
        assert_eq!(hcla.report, fixture_bytes("report.bin"));
        let report = SnpReport::parse(&hcla.report).expect("embedded report parses");
        assert_eq!(report.version, 5);
        assert_eq!(report.cpuid_family, Some(25));
        assert_eq!(report.cpuid_model, Some(17));
        assert_eq!(report.cpuid_step, Some(1));
        assert_fixture_tcb(&report);
    }

    #[test]
    fn hcla_parses_fixture_runtime_json() {
        let hcla = parse_hcla(&fixture_bytes("hcl_report.bin")).expect("fixture HCLA parses");
        assert!(
            serde_json::from_slice::<serde_json::Value>(&hcla.runtime_json)
                .expect("runtime JSON parses")
                .is_object()
        );
    }

    #[test]
    fn amd_chain_and_report_signature_accept_fixture() {
        let certificates = fixture_certificates();
        let refs: Vec<&[u8]> = certificates.iter().map(Vec::as_slice).collect();
        let verified =
            verify_amd_chain_and_report(&fixture_report(), &refs, VALID_NOW_UNIX_SECONDS)
                .expect("fixture AMD chain verifies");
        assert_eq!(verified.vcek_issuer_cn, "SEV-Genoa");
    }

    #[test]
    fn amd_report_signature_rejects_flipped_bit() {
        let mut hcla_bytes = fixture_bytes("hcl_report.bin");
        hcla_bytes[HCL_REPORT_OFFSET + SNP_OFF_MEASUREMENT] ^= 1;
        let hcla = parse_hcla(&hcla_bytes).expect("mutated HCLA still parses");
        let certificates = fixture_certificates();
        let refs: Vec<&[u8]> = certificates.iter().map(Vec::as_slice).collect();
        assert_eq!(
            verify_amd_chain_and_report(
                &SnpReport::parse(&hcla.report).expect("mutated report still parses"),
                &refs,
                VALID_NOW_UNIX_SECONDS,
            ),
            Err(SnpVerifyError::ReportSignatureInvalid)
        );
    }

    #[test]
    fn snp_report_parse_rejects_truncated_input() {
        let report = fixture_bytes("report.bin");
        assert_eq!(
            SnpReport::parse(&report[..report.len() - 1]),
            Err(SnpParseError::ReportLength)
        );
    }

    #[test]
    fn amd_chain_rejects_foreign_root_set_selection() {
        let vcek = super::parse_pem_certificate(&fixture_bytes("certs/vcek.pem"))
            .expect("fixture VCEK parses");
        assert!(matches!(
            select_root_generation(&vcek.issuer_cn, &embedded_root_pairs()[..1]),
            Err(SnpVerifyError::UnknownRootGeneration)
        ));
    }

    #[test]
    fn amd_chain_rejects_broken_root_pairing() {
        let genoa_ask = super::parse_pem_certificate(embedded_root_pairs()[1].ask_pem.as_bytes())
            .expect("embedded Genoa ASK parses");
        let milan_ark = super::parse_pem_certificate(embedded_root_pairs()[0].ark_pem.as_bytes())
            .expect("embedded Milan ARK parses");
        assert_eq!(
            verify_rsa_pss_sha384_signature(&genoa_ask.der, &milan_ark.der),
            Err(SnpVerifyError::ChainSignatureInvalid)
        );
    }

    #[test]
    fn embedded_roots_match_python_source() {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repository root")
            .to_path_buf();
        for root in embedded_root_pairs() {
            let source_root = repository_root
                .join("solstone/think/services/spp_attest/roots/amd")
                .join(root.product);
            assert_eq!(
                root.ark_pem.as_bytes(),
                fs::read(source_root.join("ark.pem")).expect("source ARK is readable")
            );
            assert_eq!(
                root.ask_pem.as_bytes(),
                fs::read(source_root.join("ask.pem")).expect("source ASK is readable")
            );
        }
    }
}
