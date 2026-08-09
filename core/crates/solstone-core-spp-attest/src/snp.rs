// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! HCLA and SEV-SNP report parsing with pinned AMD chain verification.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE};
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

use crate::{
    binding::{BINDING_DOMAIN, check_envelope_nonce, composite_binding_hash},
    error::{
        CpuAppraisalStage, CpuLegError, PcrFingerprintError, PcrPinMismatchError, SnpParseError,
        SnpVerifyError,
    },
    tlv::decode_gpu_envelope,
    tpm_quote::{TpmQuoteInput, load_ak_public_key, verify_quote, without_leading_zeros},
};

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
const SNP_POLICY_DEBUG_BIT: u32 = 19;

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

/// Minimum accepted values for one SNP TCB record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TcbFloor {
    pub boot_loader: Option<u8>,
    pub tee: Option<u8>,
    pub snp: Option<u8>,
    pub microcode: Option<u8>,
    pub fmc: Option<u8>,
}

impl TcbFloor {
    fn check(&self, observed: &TcbVersion, label: &str) -> Result<(), SnpVerifyError> {
        for (field, floor, value) in [
            ("boot_loader", self.boot_loader, observed.boot_loader),
            ("tee", self.tee, observed.tee),
            ("snp", self.snp, observed.snp),
            ("microcode", self.microcode, observed.microcode),
            ("fmc", self.fmc, observed.fmc),
        ] {
            let Some(floor) = floor else { continue };
            let Some(value) = value else {
                return Err(SnpVerifyError::PolicyTcbMissing {
                    label: label.to_owned(),
                    field,
                });
            };
            if value < floor {
                return Err(SnpVerifyError::PolicyTcbBelowFloor {
                    label: label.to_owned(),
                    field,
                    value,
                    floor,
                });
            }
        }
        Ok(())
    }
}

/// PCR acceptance mode. Unknown preserves untrusted configuration for rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcrMode {
    Record,
    Pin,
    Unknown(String),
}

/// Policy applied while appraising CPU evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub allowed_report_versions: BTreeSet<u32>,
    pub allowed_hcla_versions: BTreeSet<u32>,
    pub allowed_vmpl: BTreeSet<u32>,
    pub require_debug_disabled: bool,
    pub min_tcb: BTreeMap<String, TcbFloor>,
    pub pcr_mode: PcrMode,
    pub pcr_pins: BTreeSet<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            allowed_report_versions: [3, 5].into_iter().collect(),
            allowed_hcla_versions: [1, 2].into_iter().collect(),
            allowed_vmpl: [0].into_iter().collect(),
            require_debug_disabled: true,
            min_tcb: BTreeMap::new(),
            pcr_mode: PcrMode::Record,
            pcr_pins: BTreeSet::new(),
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

pub(crate) fn check_runtime_binding(
    report: &SnpReport,
    runtime_json: &[u8],
) -> Result<(), SnpVerifyError> {
    let runtime_digest = digest(&SHA256, runtime_json);
    if report.report_data[..32] != runtime_digest.as_ref()[..] {
        return Err(SnpVerifyError::RuntimeBindingDigestMismatch);
    }
    if report.report_data[32..].iter().any(|byte| *byte != 0) {
        return Err(SnpVerifyError::RuntimeBindingTailNonZero);
    }
    Ok(())
}

pub(crate) fn verify_ak_binding(
    runtime: &Map<String, Value>,
    ak_public_key_pem: &[u8],
) -> Result<(), SnpVerifyError> {
    let keys = runtime
        .get("keys")
        .and_then(Value::as_array)
        .ok_or(SnpVerifyError::AkJwkMissing)?;
    let jwk = keys
        .iter()
        .filter_map(Value::as_object)
        .find(|key| key.get("kid").and_then(Value::as_str) == Some("HCLAkPub"))
        .ok_or(SnpVerifyError::AkJwkMissing)?;
    let modulus = decode_jwk_component(jwk.get("n"))?;
    let exponent = decode_jwk_component(jwk.get("e"))?;
    let key = load_ak_public_key(ak_public_key_pem).map_err(|_| SnpVerifyError::AkKeyInvalid)?;
    if key.n != modulus || key.e != exponent {
        return Err(SnpVerifyError::AkKeyMismatch);
    }
    Ok(())
}

fn decode_jwk_component(value: Option<&Value>) -> Result<Vec<u8>, SnpVerifyError> {
    let value = value
        .and_then(Value::as_str)
        .ok_or(SnpVerifyError::AkJwkInvalid)?;
    let mut padded = value.to_owned();
    padded.extend(std::iter::repeat_n('=', (4 - padded.len() % 4) % 4));
    let decoded = URL_SAFE
        .decode(padded)
        .map_err(|_| SnpVerifyError::AkJwkInvalid)?;
    Ok(without_leading_zeros(&decoded))
}

fn check_policy_with(report: &SnpReport, policy: &Policy) -> Result<(), SnpVerifyError> {
    if !policy.allowed_report_versions.contains(&report.version) {
        return Err(SnpVerifyError::PolicyReportVersion);
    }
    if !policy.allowed_vmpl.is_empty() && !policy.allowed_vmpl.contains(&report.vmpl) {
        return Err(SnpVerifyError::PolicyVmpl);
    }
    if policy.require_debug_disabled && debug_allowed(report) {
        return Err(SnpVerifyError::PolicyDebugEnabled);
    }
    for (label, floor) in &policy.min_tcb {
        let observed = match label.as_str() {
            "current" => &report.current_tcb,
            "reported" => &report.reported_tcb,
            "committed" => &report.committed_tcb,
            "launch" => &report.launch_tcb,
            _ => return Err(SnpVerifyError::PolicyTcbLabelUnknown),
        };
        floor.check(observed, label)?;
    }
    Ok(())
}

fn debug_allowed(report: &SnpReport) -> bool {
    ((report.policy >> SNP_POLICY_DEBUG_BIT) & 1) == 1
}

pub(crate) fn pcr_fingerprint_hex(quote_pcrs: &[u8]) -> String {
    hex_lower(digest(&SHA256, quote_pcrs).as_ref())
}

/// Appraises CPU-leg evidence using the fixed SPP defaults.
pub fn appraise_cpu_evidence(
    evidence: CpuEvidence<'_>,
    now_unix_seconds: i64,
) -> Result<CpuAppraisal, CpuLegError> {
    appraise_cpu_leg_at(
        CpuBundle {
            hcl_report: evidence.hcl_report,
            standalone_report: evidence.standalone_report,
            cert_pems: evidence.cert_pems,
            ak_public_key_pem: evidence.ak_public_key_pem,
            nonce: evidence.nonce,
            quote_message: evidence.quote_message,
            quote_signature: evidence.quote_signature,
            quote_pcrs: evidence.quote_pcrs,
        },
        evidence.envelope_tlv,
        evidence.channel_binding,
        BINDING_DOMAIN,
        Some(&Policy::default()),
        None,
        now_unix_seconds,
    )
}

/// All CPU evidence bytes carried by composite evidence.
pub struct CpuBundle<'a> {
    pub hcl_report: &'a [u8],
    pub standalone_report: Option<&'a [u8]>,
    pub cert_pems: &'a [&'a [u8]],
    pub ak_public_key_pem: &'a [u8],
    pub nonce: &'a [u8],
    pub quote_message: &'a [u8],
    pub quote_signature: &'a [u8],
    pub quote_pcrs: &'a [u8],
}

/// Optional TPM quote verification seam for composite callers.
pub trait QuoteVerifier: Send + Sync {
    fn verify(&self, input: TpmQuoteInput<'_>) -> Result<(), crate::error::TpmQuoteError>;
}

/// Appraises CPU-leg evidence with the supplied policy or Python-equivalent defaults.
pub fn appraise_cpu_leg(
    bundle: CpuBundle<'_>,
    envelope_tlv: &[u8],
    channel_binding: &[u8],
    binding_domain: &[u8],
    policy: Option<&Policy>,
    quote_verifier: Option<&dyn QuoteVerifier>,
) -> Result<CpuAppraisal, CpuLegError> {
    let now_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    appraise_cpu_leg_at(
        bundle,
        envelope_tlv,
        channel_binding,
        binding_domain,
        policy,
        quote_verifier,
        now_unix_seconds,
    )
}

fn appraise_cpu_leg_at(
    bundle: CpuBundle<'_>,
    envelope_tlv: &[u8],
    channel_binding: &[u8],
    binding_domain: &[u8],
    policy: Option<&Policy>,
    quote_verifier: Option<&dyn QuoteVerifier>,
    now_unix_seconds: i64,
) -> Result<CpuAppraisal, CpuLegError> {
    let default_policy = Policy::default();
    let policy = policy.unwrap_or(&default_policy);
    let envelope = decode_gpu_envelope(envelope_tlv).map_err(|source| CpuLegError::Tlv {
        stage: CpuAppraisalStage::Envelope,
        source,
    })?;
    check_envelope_nonce(&envelope, bundle.nonce).map_err(|source| CpuLegError::Binding {
        stage: CpuAppraisalStage::Envelope,
        source,
    })?;

    let hcla = parse_hcla(bundle.hcl_report).map_err(|source| CpuLegError::SnpParse {
        stage: CpuAppraisalStage::Hcla,
        source,
    })?;
    if !policy.allowed_hcla_versions.contains(&hcla.version) {
        return Err(CpuLegError::SnpVerify {
            stage: CpuAppraisalStage::Hcla,
            source: SnpVerifyError::HclaVersionDisallowed,
        });
    }
    let mut steps = vec![ok_step(
        "hcla",
        format!(
            "sig=HCLA version={} request_type={}",
            hcla.version, hcla.request_type
        ),
    )];
    if bundle
        .standalone_report
        .is_some_and(|report| report != hcla.report.as_slice())
    {
        steps.push(ok_step(
            "standalone-report",
            "report.bin differs; using HCLA-embedded report".to_owned(),
        ));
    }

    let report = SnpReport::parse(&hcla.report).map_err(|source| CpuLegError::SnpParse {
        stage: CpuAppraisalStage::Report,
        source,
    })?;
    check_runtime_binding(&report, &hcla.runtime_json).map_err(|source| {
        CpuLegError::SnpVerify {
            stage: CpuAppraisalStage::RuntimeBinding,
            source,
        }
    })?;
    steps.push(ok_step(
        "runtime-binding",
        "report_data == SHA-256(runtime JSON)".to_owned(),
    ));

    let amd_verification = verify_amd_chain_and_report(&report, bundle.cert_pems, now_unix_seconds)
        .map_err(|source| CpuLegError::SnpVerify {
            stage: CpuAppraisalStage::AmdChain,
            source,
        })?;
    steps.push(ok_step(
        "amd-chain",
        format!(
            "VCEK chains to pinned {} roots",
            amd_verification.vcek_issuer_cn
        ),
    ));
    steps.push(ok_step(
        "amd-report-signature",
        "VCEK signed report bytes 0..0x29f".to_owned(),
    ));

    check_policy_with(&report, policy).map_err(|source| CpuLegError::SnpVerify {
        stage: CpuAppraisalStage::SnpPolicy,
        source,
    })?;
    steps.push(ok_step(
        "snp-policy",
        format!(
            "version={} vmpl={} debug_allowed={}",
            report.version,
            report.vmpl,
            python_bool(debug_allowed(&report))
        ),
    ));

    verify_ak_binding(&hcla.runtime, bundle.ak_public_key_pem).map_err(|source| {
        CpuLegError::SnpVerify {
            stage: CpuAppraisalStage::AkBinding,
            source,
        }
    })?;
    steps.push(ok_step(
        "ak-binding",
        "bundle AK public key matches AMD-bound HCLAkPub".to_owned(),
    ));

    let binding =
        composite_binding_hash(bundle.nonce, channel_binding, envelope_tlv, binding_domain)
            .map_err(|source| CpuLegError::Binding {
                stage: CpuAppraisalStage::Quote,
                source,
            })?;
    let quote_input = TpmQuoteInput {
        ak_public_key_pem: bundle.ak_public_key_pem,
        quote_msg: bundle.quote_message,
        quote_sig: bundle.quote_signature,
        quote_pcrs: bundle.quote_pcrs,
        expected_binding: &binding,
    };
    match quote_verifier {
        Some(verifier) => verifier.verify(quote_input),
        None => verify_quote(quote_input),
    }
    .map_err(|source| CpuLegError::TpmQuote {
        stage: CpuAppraisalStage::Quote,
        source,
    })?;
    steps.push(ok_step(
        "quote",
        "AK quote signature valid and extraData matches verifier nonce + guest key".to_owned(),
    ));

    let pcr_sha256 = check_pcr_fingerprint(bundle.quote_pcrs, policy).map_err(|source| {
        CpuLegError::PcrFingerprint {
            stage: CpuAppraisalStage::Quote,
            source,
        }
    })?;
    steps.push(ok_step(
        "pcr-policy",
        match policy.pcr_mode {
            PcrMode::Record => format!("record-then-pin v1 fingerprint={pcr_sha256}"),
            _ => format!("pinned PCR fingerprint matched {pcr_sha256}"),
        },
    ));

    Ok(CpuAppraisal {
        steps,
        hcla_version: hcla.version,
        report_version: report.version,
        cpuid_family: report.cpuid_family,
        cpuid_model: report.cpuid_model,
        cpuid_step: report.cpuid_step,
        tcb: CpuTcb {
            current: report.current_tcb,
            reported: report.reported_tcb,
            committed: report.committed_tcb,
            launch: report.launch_tcb,
        },
        pcr_sha256,
        host_data_hex: hex_lower(&report.host_data),
        measurement_hex: hex_lower(&report.measurement),
        chip_id_hex: hex_lower(&report.chip_id),
    })
}

/// Returns the SHA-256 PCR fingerprint after applying the configured policy.
pub fn check_pcr_fingerprint(
    quote_pcrs: &[u8],
    policy: &Policy,
) -> Result<String, PcrFingerprintError> {
    let fingerprint = pcr_fingerprint_hex(quote_pcrs);
    match &policy.pcr_mode {
        PcrMode::Record => Ok(fingerprint),
        PcrMode::Pin => {
            if policy
                .pcr_pins
                .iter()
                .any(|pin| pin.eq_ignore_ascii_case(&fingerprint))
            {
                Ok(fingerprint)
            } else {
                Err(PcrPinMismatchError::Mismatch { fingerprint }.into())
            }
        }
        PcrMode::Unknown(mode) => Err(PcrFingerprintError::UnknownMode { mode: mode.clone() }),
    }
}

fn ok_step(name: &'static str, detail: String) -> AppraisalStep {
    AppraisalStep {
        name,
        status: "ok",
        detail,
    }
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

fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

/// Successful AMD chain and report-signature verification details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmdReportVerification {
    pub vcek_issuer_cn: String,
}

/// All evidence bytes required for the CPU leg of SPP attestation.
pub struct CpuEvidence<'a> {
    pub hcl_report: &'a [u8],
    pub standalone_report: Option<&'a [u8]>,
    pub cert_pems: &'a [&'a [u8]],
    pub ak_public_key_pem: &'a [u8],
    pub nonce: &'a [u8],
    pub quote_message: &'a [u8],
    pub quote_signature: &'a [u8],
    pub quote_pcrs: &'a [u8],
    pub envelope_tlv: &'a [u8],
    pub channel_binding: &'a [u8],
}

/// One successful CPU-evidence appraisal check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppraisalStep {
    pub name: &'static str,
    pub status: &'static str,
    pub detail: String,
}

/// The four SNP TCB records surfaced by an accepted CPU appraisal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuTcb {
    pub current: TcbVersion,
    pub reported: TcbVersion,
    pub committed: TcbVersion,
    pub launch: TcbVersion,
}

/// The accepted CPU-leg evidence result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuAppraisal {
    pub steps: Vec<AppraisalStep>,
    pub hcla_version: u32,
    pub report_version: u32,
    pub cpuid_family: Option<u8>,
    pub cpuid_model: Option<u8>,
    pub cpuid_step: Option<u8>,
    pub tcb: CpuTcb,
    pub pcr_sha256: String,
    pub host_data_hex: String,
    pub measurement_hex: String,
    pub chip_id_hex: String,
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

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use x509_parser::pem::parse_x509_pem;

    use super::{
        CpuAppraisalStage, CpuEvidence, CpuLegError, HCL_REPORT_OFFSET, PcrMode, Policy,
        SNP_OFF_MEASUREMENT, SNP_OFF_VERSION, SnpParseError, SnpReport, SnpVerifyError,
        appraise_cpu_evidence, check_pcr_fingerprint, check_policy_with, embedded_root_pairs,
        parse_hcla, select_root_generation, verify_amd_chain_and_report,
        verify_rsa_pss_sha384_signature,
    };
    use crate::test_support::fixture_bytes;

    const VALID_NOW_UNIX_SECONDS: i64 = 1_800_000_000;

    struct FixtureInputs {
        hcl_report: Vec<u8>,
        report: Vec<u8>,
        certificates: [Vec<u8>; 3],
        ak_public_key_pem: Vec<u8>,
        nonce: Vec<u8>,
        quote_message: Vec<u8>,
        quote_signature: Vec<u8>,
        quote_pcrs: Vec<u8>,
        envelope_tlv: Vec<u8>,
        channel_binding: Vec<u8>,
    }

    fn fixture_inputs() -> FixtureInputs {
        let nonce_hex = String::from_utf8(fixture_bytes("nonce.hex")).expect("nonce is UTF-8");
        let nonce = nonce_hex
            .split_whitespace()
            .collect::<String>()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("valid hex")
            })
            .collect();
        FixtureInputs {
            hcl_report: fixture_bytes("hcl_report.bin"),
            report: fixture_bytes("report.bin"),
            certificates: fixture_certificates(),
            ak_public_key_pem: fixture_bytes("akpub.pem"),
            nonce,
            quote_message: fixture_bytes("quote.msg"),
            quote_signature: fixture_bytes("quote.sig"),
            quote_pcrs: fixture_bytes("quote.pcrs"),
            envelope_tlv: fixture_bytes("gpu-envelope.tlv"),
            channel_binding: fixture_bytes("guest_x25519.pub.der"),
        }
    }

    fn cpu_evidence<'a>(
        inputs: &'a FixtureInputs,
        cert_pems: &'a [&'a [u8]],
        standalone_report: Option<&'a [u8]>,
        ak_public_key_pem: &'a [u8],
        envelope_tlv: &'a [u8],
    ) -> CpuEvidence<'a> {
        CpuEvidence {
            hcl_report: &inputs.hcl_report,
            standalone_report,
            cert_pems,
            ak_public_key_pem,
            nonce: &inputs.nonce,
            quote_message: &inputs.quote_message,
            quote_signature: &inputs.quote_signature,
            quote_pcrs: &inputs.quote_pcrs,
            envelope_tlv,
            channel_binding: &inputs.channel_binding,
        }
    }

    fn fixture_cert_refs(inputs: &FixtureInputs) -> [&[u8]; 3] {
        [
            inputs.certificates[0].as_slice(),
            inputs.certificates[1].as_slice(),
            inputs.certificates[2].as_slice(),
        ]
    }

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

    #[test]
    fn appraise_cpu_evidence_matches_captured_fixture() {
        let inputs = fixture_inputs();
        let cert_pems = fixture_cert_refs(&inputs);
        let appraisal = appraise_cpu_evidence(
            cpu_evidence(
                &inputs,
                &cert_pems,
                Some(&inputs.report),
                &inputs.ak_public_key_pem,
                &inputs.envelope_tlv,
            ),
            VALID_NOW_UNIX_SECONDS,
        )
        .expect("fixture CPU evidence appraises");
        let steps: Vec<(&str, &str)> = appraisal
            .steps
            .iter()
            .map(|step| (step.name, step.detail.as_str()))
            .collect();
        assert_eq!(
            steps,
            vec![
                ("hcla", "sig=HCLA version=2 request_type=2"),
                ("runtime-binding", "report_data == SHA-256(runtime JSON)"),
                ("amd-chain", "VCEK chains to pinned SEV-Genoa roots"),
                ("amd-report-signature", "VCEK signed report bytes 0..0x29f"),
                ("snp-policy", "version=5 vmpl=0 debug_allowed=False"),
                (
                    "ak-binding",
                    "bundle AK public key matches AMD-bound HCLAkPub"
                ),
                (
                    "quote",
                    "AK quote signature valid and extraData matches verifier nonce + guest key",
                ),
                (
                    "pcr-policy",
                    "record-then-pin v1 fingerprint=b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3",
                ),
            ]
        );
        assert_eq!(appraisal.hcla_version, 2);
        assert_eq!(appraisal.report_version, 5);
        assert_eq!(
            (
                appraisal.cpuid_family,
                appraisal.cpuid_model,
                appraisal.cpuid_step
            ),
            (Some(25), Some(17), Some(1))
        );
        assert_eq!(
            appraisal.pcr_sha256,
            "b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3"
        );
        for tcb in [
            &appraisal.tcb.current,
            &appraisal.tcb.reported,
            &appraisal.tcb.committed,
            &appraisal.tcb.launch,
        ] {
            assert_eq!(tcb.boot_loader, Some(10));
            assert_eq!(tcb.tee, Some(0));
            assert_eq!(tcb.snp, Some(27));
            assert_eq!(tcb.microcode, Some(88));
            assert_eq!(tcb.fmc, None);
        }
    }

    #[test]
    fn appraise_cpu_evidence_rejects_tlv_splice_before_any_step() {
        let inputs = fixture_inputs();
        let cert_pems = fixture_cert_refs(&inputs);
        let mut envelope = inputs.envelope_tlv.clone();
        let nonce_start = 16;
        envelope[nonce_start] ^= 1;
        assert_eq!(
            appraise_cpu_evidence(
                cpu_evidence(
                    &inputs,
                    &cert_pems,
                    None,
                    &inputs.ak_public_key_pem,
                    &envelope,
                ),
                VALID_NOW_UNIX_SECONDS,
            ),
            Err(CpuLegError::Binding {
                stage: CpuAppraisalStage::Envelope,
                source: crate::error::BindingError::EnvelopeNonceMismatch,
            })
        );
    }

    #[test]
    fn appraise_cpu_evidence_rejects_foreign_ak_public_key() {
        let inputs = fixture_inputs();
        let cert_pems = fixture_cert_refs(&inputs);
        let (_, pem) = parse_x509_pem(&inputs.ak_public_key_pem).expect("fixture AK PEM parses");
        let mut der = pem.contents;
        let exponent = der
            .windows(3)
            .rposition(|window| window == [1, 0, 1])
            .expect("RSA exponent is present");
        der[exponent + 2] = 3;
        let foreign_ak = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(der)
        )
        .into_bytes();
        assert_eq!(
            appraise_cpu_evidence(
                cpu_evidence(&inputs, &cert_pems, None, &foreign_ak, &inputs.envelope_tlv,),
                VALID_NOW_UNIX_SECONDS,
            ),
            Err(CpuLegError::SnpVerify {
                stage: CpuAppraisalStage::AkBinding,
                source: SnpVerifyError::AkKeyMismatch,
            })
        );
    }

    #[test]
    fn appraise_cpu_evidence_records_standalone_report_difference() {
        let inputs = fixture_inputs();
        let cert_pems = fixture_cert_refs(&inputs);
        let standalone = b"not the HCLA-embedded report";
        let appraisal = appraise_cpu_evidence(
            cpu_evidence(
                &inputs,
                &cert_pems,
                Some(standalone),
                &inputs.ak_public_key_pem,
                &inputs.envelope_tlv,
            ),
            VALID_NOW_UNIX_SECONDS,
        )
        .expect("fixture CPU evidence appraises");
        assert_eq!(appraisal.steps.len(), 9);
        assert_eq!(appraisal.steps[0].name, "hcla");
        assert_eq!(appraisal.steps[1].name, "standalone-report");
    }

    #[test]
    fn check_policy_rejects_unsupported_report_version() {
        let mut report = fixture_bytes("report.bin");
        report[SNP_OFF_VERSION] = 4;
        assert_eq!(
            check_policy_with(
                &SnpReport::parse(&report).expect("mutated report parses"),
                &Policy::default(),
            ),
            Err(SnpVerifyError::PolicyReportVersion)
        );
    }

    #[test]
    fn check_policy_rejects_debug_enabled_report() {
        let report = fixture_report();
        assert_eq!(check_policy_with(&report, &Policy::default()), Ok(()));

        let mut debug_enabled = report;
        debug_enabled.policy |= 1_u64 << super::SNP_POLICY_DEBUG_BIT;
        assert_eq!(
            check_policy_with(&debug_enabled, &Policy::default()),
            Err(SnpVerifyError::PolicyDebugEnabled)
        );
    }

    #[test]
    fn default_pcr_policy_records_without_rejecting() {
        assert!(check_pcr_fingerprint(b"any PCR bytes", &Policy::default()).is_ok());
    }

    #[test]
    fn pinned_pcr_policy_compares_case_insensitively() {
        let fingerprint = super::pcr_fingerprint_hex(b"PCR bytes").to_uppercase();
        let policy = Policy {
            pcr_mode: PcrMode::Pin,
            pcr_pins: [fingerprint].into_iter().collect(),
            ..Policy::default()
        };
        assert!(check_pcr_fingerprint(b"PCR bytes", &policy).is_ok());
    }

    #[test]
    fn pinned_pcr_policy_reports_a_distinct_mismatch() {
        let policy = Policy {
            pcr_mode: PcrMode::Pin,
            pcr_pins: ["00".repeat(32)].into_iter().collect(),
            ..Policy::default()
        };
        assert!(matches!(
            check_pcr_fingerprint(b"PCR bytes", &policy),
            Err(crate::error::PcrFingerprintError::PinMismatch(_))
        ));
    }

    #[test]
    fn unknown_pcr_policy_mode_rejects() {
        let policy = Policy {
            pcr_mode: PcrMode::Unknown("surprise".to_owned()),
            ..Policy::default()
        };
        assert!(matches!(
            check_pcr_fingerprint(b"PCR bytes", &policy),
            Err(crate::error::PcrFingerprintError::UnknownMode { .. })
        ));
    }
}
