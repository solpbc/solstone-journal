// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-language evidence-layer differential against the running Python implementation.
//!
//! The broken-root-pairing and foreign-root-selection cases are intentionally excluded. Python
//! exposes a test-only root-directory override for those cases, while the Rust production API
//! deliberately does not expose a trust-root override: such a knob would weaken the pinned-root,
//! fail-closed design. Both implementations cover the equivalent internal paths in same-language
//! unit tests.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value, json};
use solstone_core_spp_attest::{
    binding::{BINDING_DOMAIN, check_envelope_nonce, composite_binding_hash},
    nvgpu::{
        NvattestVerdict, build_gpu_appraisal, classify_nvattest_result, parse_nvattest_stdout,
    },
    snp::{CpuEvidence, SnpReport, appraise_cpu_evidence},
    tlv::decode_gpu_envelope,
    tpm_quote::{TpmQuoteInput, verify_quote},
};

const CASES: [&str; 16] = [
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

const PYTHON_ORACLE: &str = r#"
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, os.environ["SOLSTONE_REPO_ROOT"])

from solstone.think.services.spp_attest import (
    BINDING_DOMAIN,
    composite_binding_hash,
)
from solstone.think.services.spp_attest.binding import check_envelope_nonce
from solstone.think.services.spp_attest.nvgpu.claims import (
    NvattestAcceptance,
    NvattestRejection,
    build_gpu_appraisal,
    classify_nvattest_result,
    parse_nvattest_stdout,
)
from solstone.think.services.spp_attest.tpm_quote import verify_quote
from solstone.think.services.spp_attest.snp import SnpReport, appraise_cpu_leg, load_cpu_bundle
from solstone.think.services.spp_attest.tlv import decode_gpu_envelope


def owner_nonce(root):
    return bytes.fromhex("".join((root / "nonce.hex").read_text(encoding="utf-8").split()))


def binding(root):
    return composite_binding_hash(
        nonce=owner_nonce(root),
        channel_binding=(root / "guest_x25519.pub.der").read_bytes(),
        envelope_tlv=(root / "gpu-envelope.tlv").read_bytes(),
        domain=BINDING_DOMAIN,
    )


def accepted(kind, **fields):
    return {"case": kind, "status": "accepted", **fields}


def rejected(kind, **fields):
    return {"case": kind, "status": "rejected", **fields}


def gpu_verdict(kind, root, stdout_name, unknown_result_code=False):
    stdout = parse_nvattest_stdout((root / "nvattest" / stdout_name).read_text(encoding="utf-8"))
    if unknown_result_code:
        stdout["result_code"] = 999
    verdict = classify_nvattest_result(0, stdout, owner_nonce=owner_nonce(root))
    if isinstance(verdict, NvattestRejection):
        return rejected(kind, reason=verdict.reason)
    assert isinstance(verdict, NvattestAcceptance)
    appraisal = build_gpu_appraisal(
        claim=verdict.claim,
        envelope=decode_gpu_envelope((root / "gpu-envelope.tlv").read_bytes()),
        steps=[],
    )
    return accepted(
        kind,
        driver_version=appraisal.driver_version,
        vbios_version=appraisal.vbios_version,
        hwmodel=appraisal.hwmodel,
        arch=appraisal.arch,
        envelope_gpu_uuid=appraisal.envelope_gpu_uuid,
    )


class UnknownCase(Exception):
    pass


def evaluate(case):
    kind = case["kind"]
    root = Path(case["root"])
    try:
        if kind == "snp_report_parse" or kind == "snp_report_truncated":
            report = SnpReport.parse((root / "report.bin").read_bytes())
            return accepted(
                kind,
                report_version=report.version,
                cpuid=[report.cpuid_family, report.cpuid_model, report.cpuid_step],
            )
        if kind == "cpu_leg_positive" or kind == "snp_signature_bit_flip":
            result = appraise_cpu_leg(
                load_cpu_bundle(root),
                envelope_tlv=(root / "gpu-envelope.tlv").read_bytes(),
                channel_binding=(root / "guest_x25519.pub.der").read_bytes(),
            )
            return accepted(kind, steps=[step.name for step in result.steps])
        if kind == "tlv_decode_positive" or kind == "tlv_envelope_truncated":
            decode_gpu_envelope((root / "gpu-envelope.tlv").read_bytes())
            return accepted(kind)
        if kind == "binding_hash_positive":
            return accepted(kind, digest=binding(root).hex())
        if kind == "tpm_quote_positive" or kind == "tpm_signature_bit_flip" or kind == "tpm_quote_truncated":
            verify_quote(
                ak_pub_pem=(root / "akpub.pem").read_bytes(),
                quote_msg=(root / "quote.msg").read_bytes(),
                quote_sig=(root / "quote.sig").read_bytes(),
                quote_pcrs=(root / "quote.pcrs").read_bytes(),
                expected_binding=binding(root),
            )
            return accepted(kind)
        if kind == "envelope_nonce_mismatch":
            check_envelope_nonce(
                decode_gpu_envelope((root / "gpu-envelope.tlv").read_bytes()),
                owner_nonce(root),
            )
            return accepted(kind)
        if kind == "gpu_positive":
            return gpu_verdict(kind, root, "positive.stdout")
        if kind == "gpu_neg_a":
            return gpu_verdict(kind, root, "negA.stdout")
        if kind == "gpu_neg_b":
            return gpu_verdict(kind, root, "negB.stdout")
        if kind == "gpu_neg_c":
            return gpu_verdict(kind, root, "negC.stdout")
        if kind == "gpu_unknown_result_code":
            return gpu_verdict(kind, root, "positive.stdout", unknown_result_code=True)
        raise UnknownCase(kind)
    except UnknownCase:
        # Never convert this into a verdict. It is not a rejection the
        # reference computed -- it is the oracle admitting it does not implement
        # the case, and `comparable` strips `exception_type`, so a swallowed
        # UnknownCase is indistinguishable from a real rejection and the
        # differential passes while testing nothing.
        raise
    except Exception as exc:
        return rejected(kind, exception_type=type(exc).__name__)


print(json.dumps(evaluate(json.loads(sys.argv[1])), sort_keys=True))
"#;

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
            "solstone-spp-attest-differential-{case}-{}-{stamp}",
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

fn python() -> PathBuf {
    let venv = repository_root().join(".venv/bin/python3");
    assert!(venv.is_file(), "differential requires make install");
    venv
}

fn python_verdict(kind: &str, root: &Path) -> Value {
    let case = json!({"kind": kind, "root": root});
    let output = Command::new(python())
        .arg("-c")
        .arg(PYTHON_ORACLE)
        .arg(case.to_string())
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python SPP attestation oracle");
    assert!(
        output.status.success(),
        "Python stderr: {:?}",
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).expect("Python verdict JSON")
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

fn rust_verdict(kind: &str, root: &Path) -> Value {
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
        _ => panic!("unknown differential case: {kind}"),
    }
}

fn expected_status(kind: &str) -> &'static str {
    match kind {
        "snp_report_parse"
        | "cpu_leg_positive"
        | "tlv_decode_positive"
        | "binding_hash_positive"
        | "tpm_quote_positive"
        | "gpu_positive" => "accepted",
        _ => "rejected",
    }
}

fn expected_reason(kind: &str) -> Option<&'static str> {
    match kind {
        "gpu_neg_a" | "gpu_neg_b" | "gpu_neg_c" => Some("gpu_nonce_mismatch"),
        "gpu_unknown_result_code" => Some("gpu_appraisal_failed"),
        _ => None,
    }
}

fn comparable(mut verdict: Value) -> Value {
    verdict
        .as_object_mut()
        .expect("verdict is an object")
        .remove("exception_type");
    verdict
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

#[test]
fn spp_attest_matches_python_oracle() {
    assert!(!CASES.is_empty(), "differential corpus must not be empty");
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
        let rust = rust_verdict(kind, &root);
        let python = python_verdict(kind, &root);
        assert_eq!(
            rust.get("status").and_then(Value::as_str),
            Some(expected_status(kind)),
            "Rust case={kind}"
        );
        assert_eq!(
            python.get("status").and_then(Value::as_str),
            Some(expected_status(kind)),
            "Python case={kind}"
        );
        if let Some(expected_reason) = expected_reason(kind) {
            assert_eq!(
                rust.get("reason").and_then(Value::as_str),
                Some(expected_reason),
                "Rust case={kind}"
            );
            assert_eq!(
                python.get("reason").and_then(Value::as_str),
                Some(expected_reason),
                "Python case={kind}"
            );
        }
        assert_eq!(comparable(rust), comparable(python), "case={kind}");
        executed += 1;
    }
    assert_eq!(executed, CASES.len(), "every corpus case must execute");
}
