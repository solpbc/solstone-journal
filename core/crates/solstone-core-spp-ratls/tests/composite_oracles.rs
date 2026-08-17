// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native composite CPU/GPU attestation corpus.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};
use solstone_core_spp_attest::{
    binding::BINDING_DOMAIN,
    error::GpuAppraisalReason,
    nvgpu::{
        build_gpu_appraisal, classify_nvattest_result, parse_nvattest_stdout, GpuAppraisal,
        NvattestVerdict,
    },
    tlv::GpuEnvelope,
    CpuBundle, GpuAppraiser, PcrMode, Policy,
};
use solstone_core_spp_ratls::{verify_composite_with_gpu_appraiser, CompositeVerificationInput};

const CASES: [&str; 5] = [
    "composite_positive",
    "composite_gpu_reject",
    "composite_cpu_reject_tampered_binding",
    "composite_pin_mismatch",
    "composite_gpu_prerequisite_reject",
];

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
            "solstone-composite-differential-{case}-{}-{stamp}",
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

struct FixtureInputs {
    nonce: [u8; 32],
    hcl_report: Vec<u8>,
    report: Vec<u8>,
    ark: Vec<u8>,
    ask: Vec<u8>,
    vcek: Vec<u8>,
    ak: Vec<u8>,
    quote_message: Vec<u8>,
    quote_signature: Vec<u8>,
    quote_pcrs: Vec<u8>,
    envelope: Vec<u8>,
    channel_binding: Vec<u8>,
}

impl FixtureInputs {
    fn load(root: &Path) -> Self {
        Self {
            nonce: nonce(root),
            hcl_report: read(root, "hcl_report.bin"),
            report: read(root, "report.bin"),
            ark: read(root, "certs/ark.pem"),
            ask: read(root, "certs/ask.pem"),
            vcek: read(root, "certs/vcek.pem"),
            ak: read(root, "akpub.pem"),
            quote_message: read(root, "quote.msg"),
            quote_signature: read(root, "quote.sig"),
            quote_pcrs: read(root, "quote.pcrs"),
            envelope: read(root, "gpu-envelope.tlv"),
            channel_binding: read(root, "guest_x25519.pub.der"),
        }
    }
}

struct FixtureGpuAppraiser {
    stdout: Option<String>,
    rejection: Option<GpuAppraisalReason>,
    called: AtomicBool,
}

impl FixtureGpuAppraiser {
    fn for_case(kind: &str, root: &Path) -> Self {
        let stdout = match kind {
            "composite_positive"
            | "composite_cpu_reject_tampered_binding"
            | "composite_pin_mismatch" => Some(
                String::from_utf8(read(root, "nvattest/positive.stdout")).expect("UTF-8 stdout"),
            ),
            "composite_gpu_reject" => {
                Some(String::from_utf8(read(root, "nvattest/negA.stdout")).expect("UTF-8 stdout"))
            }
            "composite_gpu_prerequisite_reject" => None,
            _ => panic!("unknown case: {kind}"),
        };
        let rejection = (kind == "composite_gpu_prerequisite_reject")
            .then_some(GpuAppraisalReason::NvattestUnavailable);
        Self {
            stdout,
            rejection,
            called: AtomicBool::new(false),
        }
    }
}

impl GpuAppraiser for FixtureGpuAppraiser {
    fn appraise(
        &self,
        envelope: &GpuEnvelope,
        owner_nonce: &[u8; 32],
        _: &Path,
    ) -> Result<GpuAppraisal, GpuAppraisalReason> {
        self.called.store(true, Ordering::SeqCst);
        if let Some(reason) = self.rejection {
            return Err(reason);
        }
        let stdout = parse_nvattest_stdout(self.stdout.as_deref().expect("fixture stdout"))
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        match classify_nvattest_result(0, &stdout, owner_nonce) {
            NvattestVerdict::Accepted(acceptance) => {
                build_gpu_appraisal(&acceptance.claim, envelope, Vec::new())
                    .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)
            }
            NvattestVerdict::Rejected(rejection) => Err(rejection.reason),
        }
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

fn rust_verdict(kind: &str, root: &Path) -> Value {
    let inputs = FixtureInputs::load(root);
    let certificates = [&inputs.ark[..], &inputs.ask[..], &inputs.vcek[..]];
    let policy = (kind == "composite_pin_mismatch").then(|| Policy {
        pcr_mode: PcrMode::Pin,
        pcr_pins: ["00".repeat(32)].into_iter().collect(),
        ..Policy::default()
    });
    let appraiser = FixtureGpuAppraiser::for_case(kind, root);
    let result = verify_composite_with_gpu_appraiser(
        CpuBundle {
            hcl_report: &inputs.hcl_report,
            standalone_report: Some(&inputs.report),
            cert_pems: &certificates,
            ak_public_key_pem: &inputs.ak,
            nonce: &inputs.nonce,
            quote_message: &inputs.quote_message,
            quote_signature: &inputs.quote_signature,
            quote_pcrs: &inputs.quote_pcrs,
        },
        CompositeVerificationInput {
            envelope_tlv: &inputs.envelope,
            channel_binding: &inputs.channel_binding,
            owner_nonce: &inputs.nonce,
            now: SystemTime::UNIX_EPOCH,
            nvattest_dir: root,
            binding_domain: BINDING_DOMAIN,
            roots_dir: None,
            policy: policy.as_ref(),
            quote_verifier: None,
        },
        &appraiser,
        root,
        SystemTime::UNIX_EPOCH,
    );
    match result {
        Ok(verdict) => json!({
            "case": kind,
            "status": "accepted",
            "verified": verdict.verified,
            "legs": verdict.legs,
            "substrate": verdict.substrate,
            "gpu_called": appraiser.called.load(Ordering::SeqCst),
        }),
        Err(error) => json!({
            "case": kind,
            "status": "rejected",
            "reason": error.reason_code,
            "gpu_called": appraiser.called.load(Ordering::SeqCst),
        }),
    }
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

fn mutate_fixture(kind: &str, root: &Path) {
    if kind != "composite_cpu_reject_tampered_binding" {
        return;
    }
    let path = root.join("guest_x25519.pub.der");
    let mut binding = fs::read(&path).expect("read channel binding");
    binding[0] ^= 1;
    fs::write(path, binding).expect("write tampered channel binding");
}

fn read(root: &Path, name: &str) -> Vec<u8> {
    fs::read(root.join(name)).expect("read fixture")
}

fn nonce(root: &Path) -> [u8; 32] {
    let hex = String::from_utf8(read(root, "nonce.hex")).expect("nonce UTF-8");
    hex.split_whitespace()
        .collect::<String>()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("byte"))
        .collect::<Vec<_>>()
        .try_into()
        .expect("32-byte nonce")
}

fn expected_status(kind: &str) -> &'static str {
    match kind {
        "composite_positive" => "accepted",
        _ => "rejected",
    }
}

fn expected_reason(kind: &str) -> Option<&'static str> {
    match kind {
        "composite_gpu_reject" => Some("gpu_nonce_mismatch"),
        "composite_cpu_reject_tampered_binding" => Some("cpu_verification_failed"),
        "composite_pin_mismatch" => Some("pcr_pin_mismatch"),
        "composite_gpu_prerequisite_reject" => Some("nvattest_unavailable"),
        _ => None,
    }
}

fn expected_gpu_called(kind: &str) -> bool {
    matches!(
        kind,
        "composite_positive" | "composite_gpu_reject" | "composite_gpu_prerequisite_reject"
    )
}

#[test]
fn composite_fixture_corpus_matches_the_accept_reject_table() {
    assert_eq!(CASES.len(), 5, "the composite corpus lost a case");
    for case in CASES {
        let fixture = TempFixture::copy_from(&fixture_root(), case);
        mutate_fixture(case, &fixture.path);
        let rust = rust_verdict(case, &fixture.path);
        assert_eq!(
            rust.get("status").and_then(Value::as_str),
            Some(expected_status(case)),
            "case={case} verdict={rust}"
        );
        if let Some(reason) = expected_reason(case) {
            assert_eq!(
                rust.get("reason").and_then(Value::as_str),
                Some(reason),
                "case={case} verdict={rust}"
            );
        }
        assert_eq!(
            rust.get("gpu_called").and_then(Value::as_bool),
            Some(expected_gpu_called(case)),
            "case={case} verdict={rust}"
        );
        if expected_status(case) == "accepted" {
            assert_eq!(rust.get("verified").and_then(Value::as_bool), Some(true));
            assert_eq!(rust.get("legs"), Some(&json!(["cpu", "gpu"])));
        }
    }
}
