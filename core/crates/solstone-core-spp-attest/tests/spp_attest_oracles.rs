// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native evidence-layer corpus for SPP attestation fixtures.
//!
//! The broken-root-pairing and foreign-root-selection cases are intentionally excluded. Python
//! exposes a test-only root-directory override for those cases, while the Rust production API
//! deliberately does not expose a trust-root override: such a knob would weaken the pinned-root,
//! fail-closed design. Both implementations cover the equivalent internal paths in same-language
//! unit tests.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Map, Value};
use solstone_core_spp_attest::{
    binding::{check_envelope_nonce, composite_binding_hash, BINDING_DOMAIN},
    nvgpu::{
        build_gpu_appraisal, classify_nvattest_result, parse_nvattest_stdout, NvattestVerdict,
    },
    snp::{appraise_cpu_evidence, CpuEvidence, SnpReport},
    tlv::decode_gpu_envelope,
    tpm_quote::{verify_quote, TpmQuoteInput},
};

// Case names from the committed tests/fixtures/spp_attest corpus, recorded
// against native_verdict on 2026-08-16.
const CASES: &[&str] = &[
    "snp_report_parse",
    "cpu_leg_positive",
    "tlv_decode_positive",
    "binding_hash_positive",
    "tpm_quote_positive",
    "gpu_positive",
    "gpu_neg_a",
    "gpu_neg_b",
    "gpu_neg_c",
    "snp_signature_bit_flip",
    "tpm_signature_bit_flip",
    "snp_report_truncated",
    "tlv_envelope_truncated",
    "tpm_quote_truncated",
    "envelope_nonce_mismatch",
    "gpu_unknown_result_code",
];

const VALID_NOW_UNIX_SECONDS: i64 = 1_800_000_000;

struct TempFixture {
    path: PathBuf,
}

impl TempFixture {
    fn copy_from(source: &Path, case: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time is available")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-spp-attest-oracles-{case}-{}-{stamp}",
            std::process::id()
        ));
        copy_tree(source, &path);
        Self { path }
    }
}

impl Drop for TempFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn fixture_root() -> PathBuf {
    repository_root().join("tests/fixtures/spp_attest")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied fixture directory");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().expect("read fixture type").is_dir() {
            copy_tree(&entry.path(), &destination_path);
        } else {
            fs::copy(entry.path(), destination_path).expect("copy fixture file");
        }
    }
}

fn mutation_case(kind: &str) -> bool {
    matches!(
        kind,
        "snp_signature_bit_flip"
            | "tpm_signature_bit_flip"
            | "snp_report_truncated"
            | "tlv_envelope_truncated"
            | "tpm_quote_truncated"
            | "envelope_nonce_mismatch"
    )
}

fn mutate_fixture(kind: &str, root: &Path) {
    let (file, offset) = match kind {
        "snp_signature_bit_flip" => ("hcl_report.bin", 32 + 0x90),
        "tpm_signature_bit_flip" => {
            let path = root.join("quote.sig");
            let mut bytes = fs::read(&path).expect("read quote signature");
            let last = bytes.last_mut().expect("fixture signature is nonempty");
            *last ^= 1;
            fs::write(path, bytes).expect("write copied quote signature");
            return;
        }
        "snp_report_truncated" => ("report.bin", usize::MAX),
        "tlv_envelope_truncated" => ("gpu-envelope.tlv", usize::MAX),
        "tpm_quote_truncated" => ("quote.msg", usize::MAX),
        "envelope_nonce_mismatch" => ("gpu-envelope.tlv", 16),
        _ => panic!("not a mutation case: {kind}"),
    };
    let path = root.join(file);
    let mut bytes = fs::read(&path).expect("read copied fixture");
    if offset == usize::MAX {
        bytes.pop().expect("fixture is nonempty");
    } else {
        bytes[offset] ^= 1;
    }
    fs::write(path, bytes).expect("write copied fixture");
}

fn nonce(root: &Path) -> [u8; 32] {
    let hex = fs::read_to_string(root.join("nonce.hex")).expect("read nonce fixture");
    let bytes = hex
        .split_whitespace()
        .collect::<String>()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                .expect("valid nonce hex")
        })
        .collect::<Vec<_>>();
    bytes.try_into().expect("fixture nonce is 32 bytes")
}

fn binding(root: &Path) -> [u8; 32] {
    composite_binding_hash(
        &nonce(root),
        &fs::read(root.join("guest_x25519.pub.der")).expect("read channel binding"),
        &fs::read(root.join("gpu-envelope.tlv")).expect("read envelope"),
        BINDING_DOMAIN,
    )
    .expect("fixture binding computes")
}

fn cpu_appraisal_steps(root: &Path) -> Result<Vec<String>, ()> {
    let hcl_report = fs::read(root.join("hcl_report.bin")).expect("read HCLA fixture");
    let standalone_report = fs::read(root.join("report.bin")).ok();
    let certificates = [
        fs::read(root.join("certs/ark.pem")).expect("read ARK"),
        fs::read(root.join("certs/ask.pem")).expect("read ASK"),
        fs::read(root.join("certs/vcek.pem")).expect("read VCEK"),
    ];
    let certificate_refs = certificates.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let ak_public_key_pem = fs::read(root.join("akpub.pem")).expect("read AK public key");
    let quote_message = fs::read(root.join("quote.msg")).expect("read quote message");
    let quote_signature = fs::read(root.join("quote.sig")).expect("read quote signature");
    let quote_pcrs = fs::read(root.join("quote.pcrs")).expect("read quote PCRs");
    let envelope_tlv = fs::read(root.join("gpu-envelope.tlv")).expect("read envelope");
    let channel_binding =
        fs::read(root.join("guest_x25519.pub.der")).expect("read channel binding");
    appraise_cpu_evidence(
        CpuEvidence {
            hcl_report: &hcl_report,
            standalone_report: standalone_report.as_deref(),
            cert_pems: &certificate_refs,
            ak_public_key_pem: &ak_public_key_pem,
            nonce: &nonce(root),
            quote_message: &quote_message,
            quote_signature: &quote_signature,
            quote_pcrs: &quote_pcrs,
            envelope_tlv: &envelope_tlv,
            channel_binding: &channel_binding,
        },
        VALID_NOW_UNIX_SECONDS,
    )
    .map(|appraisal| {
        appraisal
            .steps
            .into_iter()
            .map(|step| step.name.to_owned())
            .collect()
    })
    .map_err(|_| ())
}

fn gpu_reason_name(reason: solstone_core_spp_attest::error::GpuAppraisalReason) -> &'static str {
    match reason {
        solstone_core_spp_attest::error::GpuAppraisalReason::NvattestUnavailable => {
            "nvattest_unavailable"
        }
        solstone_core_spp_attest::error::GpuAppraisalReason::NvattestIntegrityFailed => {
            "nvattest_integrity_failed"
        }
        solstone_core_spp_attest::error::GpuAppraisalReason::GpuNonceMismatch => {
            "gpu_nonce_mismatch"
        }
        solstone_core_spp_attest::error::GpuAppraisalReason::GpuAppraisalFailed => {
            "gpu_appraisal_failed"
        }
    }
}

fn accepted(kind: &str, fields: Map<String, Value>) -> Value {
    let mut verdict = Map::new();
    verdict.insert("case".to_owned(), json!(kind));
    verdict.insert("status".to_owned(), json!("accepted"));
    verdict.extend(fields);
    Value::Object(verdict)
}

fn rejected(kind: &str, reason: Option<&str>) -> Value {
    let mut verdict = Map::new();
    verdict.insert("case".to_owned(), json!(kind));
    verdict.insert("status".to_owned(), json!("rejected"));
    if let Some(reason) = reason {
        verdict.insert("reason".to_owned(), json!(reason));
    }
    Value::Object(verdict)
}

fn native_verdict(kind: &str, root: &Path) -> Value {
    match kind {
        "snp_report_parse" | "snp_report_truncated" => {
            match SnpReport::parse(&fs::read(root.join("report.bin")).expect("report")) {
                Ok(report) => accepted(
                    kind,
                    Map::from_iter([
                        ("report_version".to_owned(), json!(report.version)),
                        (
                            "cpuid".to_owned(),
                            json!([report.cpuid_family, report.cpuid_model, report.cpuid_step]),
                        ),
                    ]),
                ),
                Err(_) => rejected(kind, None),
            }
        }
        "cpu_leg_positive" | "snp_signature_bit_flip" => match cpu_appraisal_steps(root) {
            Ok(steps) => accepted(kind, Map::from_iter([("steps".to_owned(), json!(steps))])),
            Err(()) => rejected(kind, None),
        },
        "tlv_decode_positive" | "tlv_envelope_truncated" => {
            if decode_gpu_envelope(&fs::read(root.join("gpu-envelope.tlv")).expect("envelope"))
                .is_ok()
            {
                accepted(kind, Map::new())
            } else {
                rejected(kind, None)
            }
        }
        "binding_hash_positive" => match composite_binding_hash(
            &nonce(root),
            &fs::read(root.join("guest_x25519.pub.der")).expect("channel binding"),
            &fs::read(root.join("gpu-envelope.tlv")).expect("envelope"),
            BINDING_DOMAIN,
        ) {
            Ok(digest) => accepted(
                kind,
                Map::from_iter([("digest".to_owned(), json!(hex_lower(&digest)))]),
            ),
            Err(_) => rejected(kind, None),
        },
        "tpm_quote_positive" | "tpm_signature_bit_flip" | "tpm_quote_truncated" => {
            if verify_quote(TpmQuoteInput {
                ak_public_key_pem: &fs::read(root.join("akpub.pem")).expect("AK"),
                quote_msg: &fs::read(root.join("quote.msg")).expect("quote message"),
                quote_sig: &fs::read(root.join("quote.sig")).expect("quote signature"),
                quote_pcrs: &fs::read(root.join("quote.pcrs")).expect("quote PCRs"),
                expected_binding: &binding(root),
            })
            .is_ok()
            {
                accepted(kind, Map::new())
            } else {
                rejected(kind, None)
            }
        }
        "envelope_nonce_mismatch" => {
            match decode_gpu_envelope(&fs::read(root.join("gpu-envelope.tlv")).expect("envelope")) {
                Ok(envelope) if check_envelope_nonce(&envelope, &nonce(root)).is_ok() => {
                    accepted(kind, Map::new())
                }
                Ok(_) | Err(_) => rejected(kind, None),
            }
        }
        "gpu_positive" | "gpu_neg_a" | "gpu_neg_b" | "gpu_neg_c" | "gpu_unknown_result_code" => {
            let stdout_name = match kind {
                "gpu_neg_a" => "negA.stdout",
                "gpu_neg_b" => "negB.stdout",
                "gpu_neg_c" => "negC.stdout",
                _ => "positive.stdout",
            };
            let mut stdout = match parse_nvattest_stdout(
                &fs::read_to_string(root.join("nvattest").join(stdout_name)).expect("GPU stdout"),
            ) {
                Ok(stdout) => stdout,
                Err(_) => return rejected(kind, None),
            };
            if kind == "gpu_unknown_result_code" {
                stdout
                    .as_object_mut()
                    .expect("positive stdout is an object")
                    .insert("result_code".to_owned(), json!(999));
            }
            match classify_nvattest_result(0, &stdout, &nonce(root)) {
                NvattestVerdict::Rejected(rejection) => {
                    rejected(kind, Some(gpu_reason_name(rejection.reason)))
                }
                NvattestVerdict::Accepted(acceptance) => {
                    let envelope = match decode_gpu_envelope(
                        &fs::read(root.join("gpu-envelope.tlv")).expect("envelope"),
                    ) {
                        Ok(envelope) => envelope,
                        Err(_) => return rejected(kind, None),
                    };
                    match build_gpu_appraisal(&acceptance.claim, &envelope, Vec::new()) {
                        Ok(appraisal) => accepted(
                            kind,
                            Map::from_iter([
                                ("driver_version".to_owned(), json!(appraisal.driver_version)),
                                ("vbios_version".to_owned(), json!(appraisal.vbios_version)),
                                ("hwmodel".to_owned(), json!(appraisal.hwmodel)),
                                ("arch".to_owned(), json!(appraisal.arch)),
                                (
                                    "envelope_gpu_uuid".to_owned(),
                                    json!(appraisal.envelope_gpu_uuid),
                                ),
                            ]),
                        ),
                        Err(_) => rejected(kind, None),
                    }
                }
            }
        }
        _ => panic!("unknown case: {kind}"),
    }
}

// comparable()-shaped verdicts recorded from native_verdict on 2026-08-16.
fn expected_verdict(kind: &str) -> Value {
    match kind {
        "snp_report_parse" => json!({
            "case": "snp_report_parse",
            "status": "accepted",
            "report_version": 5,
            "cpuid": [25, 17, 1],
        }),
        "cpu_leg_positive" => json!({
            "case": "cpu_leg_positive",
            "status": "accepted",
            "steps": [
                "hcla",
                "runtime-binding",
                "amd-chain",
                "amd-report-signature",
                "snp-policy",
                "ak-binding",
                "quote",
                "pcr-policy",
            ],
        }),
        "tlv_decode_positive" => json!({
            "case": "tlv_decode_positive",
            "status": "accepted",
        }),
        "binding_hash_positive" => json!({
            "case": "binding_hash_positive",
            "status": "accepted",
            "digest": "268901922d7b8444139f3d3e3edfcc3dd860491e313b243d94fb97ba5b312ea2",
        }),
        "tpm_quote_positive" => json!({
            "case": "tpm_quote_positive",
            "status": "accepted",
        }),
        "gpu_positive" => json!({
            "case": "gpu_positive",
            "status": "accepted",
            "driver_version": "595.71.05",
            "vbios_version": "96.00.88.00.11",
            "hwmodel": "GH100 A01 GSP BROM",
            "arch": "HOPPER",
            "envelope_gpu_uuid": "GPU-256cc88f-e93b-9396-b581-274543ea3235",
        }),
        "gpu_neg_a" => json!({
            "case": "gpu_neg_a",
            "status": "rejected",
            "reason": "gpu_nonce_mismatch",
        }),
        "gpu_neg_b" => json!({
            "case": "gpu_neg_b",
            "status": "rejected",
            "reason": "gpu_nonce_mismatch",
        }),
        "gpu_neg_c" => json!({
            "case": "gpu_neg_c",
            "status": "rejected",
            "reason": "gpu_nonce_mismatch",
        }),
        "snp_signature_bit_flip" => json!({
            "case": "snp_signature_bit_flip",
            "status": "rejected",
        }),
        "tpm_signature_bit_flip" => json!({
            "case": "tpm_signature_bit_flip",
            "status": "rejected",
        }),
        "snp_report_truncated" => json!({
            "case": "snp_report_truncated",
            "status": "rejected",
        }),
        "tlv_envelope_truncated" => json!({
            "case": "tlv_envelope_truncated",
            "status": "rejected",
        }),
        "tpm_quote_truncated" => json!({
            "case": "tpm_quote_truncated",
            "status": "rejected",
        }),
        "envelope_nonce_mismatch" => json!({
            "case": "envelope_nonce_mismatch",
            "status": "rejected",
        }),
        "gpu_unknown_result_code" => json!({
            "case": "gpu_unknown_result_code",
            "status": "rejected",
            "reason": "gpu_appraisal_failed",
        }),
        _ => panic!("unknown case: {kind}"),
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

fn assert_recorded_fields_match_fixtures() {
    let root = fixture_root();
    let report = SnpReport::parse(&fs::read(root.join("report.bin")).expect("report.bin"))
        .expect("SNP fixture parses");
    let recorded = expected_verdict("snp_report_parse");
    assert_eq!(recorded["report_version"], json!(report.version));
    assert_eq!(
        recorded["cpuid"],
        json!([report.cpuid_family, report.cpuid_model, report.cpuid_step])
    );

    let recorded_binding = expected_verdict("binding_hash_positive");
    let digest = recorded_binding["digest"]
        .as_str()
        .expect("recorded digest");
    assert_eq!(digest.len(), 64, "binding digest must be 64 hex chars");
    assert!(
        digest.chars().all(|ch| ch.is_ascii_hexdigit()),
        "binding digest must be hex: {digest}"
    );

    let envelope =
        decode_gpu_envelope(&fs::read(root.join("gpu-envelope.tlv")).expect("envelope fixture"))
            .expect("GPU envelope fixture decodes");
    let recorded_gpu = expected_verdict("gpu_positive");
    assert_eq!(
        recorded_gpu["envelope_gpu_uuid"],
        json!(std::str::from_utf8(envelope.field(6).expect("uuid field")).expect("uuid utf-8"))
    );
    assert_eq!(
        recorded_gpu["arch"],
        json!(std::str::from_utf8(envelope.field(7).expect("arch field"))
            .expect("arch utf-8")
            .to_uppercase())
    );
    let stdout =
        fs::read_to_string(root.join("nvattest/positive.stdout")).expect("positive GPU stdout");
    for field in ["driver_version", "vbios_version", "hwmodel"] {
        let value = recorded_gpu[field].as_str().expect("recorded GPU field");
        assert!(
            stdout.contains(value),
            "recorded {field}={value} is not in the nvattest fixture"
        );
    }
}

#[test]
fn spp_attest_fixture_corpus_matches_the_accept_reject_table() {
    assert_eq!(CASES.len(), 16, "the SPP attestation corpus lost a case");
    assert_recorded_fields_match_fixtures();
    let mut executed = 0;
    for kind in CASES {
        let temporary = mutation_case(kind).then(|| {
            let temporary = TempFixture::copy_from(&fixture_root(), kind);
            mutate_fixture(kind, &temporary.path);
            temporary
        });
        let root = temporary
            .as_ref()
            .map_or_else(fixture_root, |temporary| temporary.path.clone());
        let native = native_verdict(kind, &root);
        assert_eq!(
            native,
            expected_verdict(kind),
            "case={kind} verdict={native}"
        );
        executed += 1;
    }
    assert_eq!(executed, CASES.len(), "every corpus case must execute");
}
