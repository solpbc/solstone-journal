// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-language differential for composite CPU/GPU attestation orchestration.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use solstone_core_spp_attest::{
    CpuBundle, GpuAppraiser, PcrMode, Policy,
    binding::BINDING_DOMAIN,
    error::GpuAppraisalReason,
    nvgpu::{
        GpuAppraisal, NvattestVerdict, build_gpu_appraisal, classify_nvattest_result,
        parse_nvattest_stdout,
    },
    tlv::GpuEnvelope,
};
use solstone_core_spp_ratls::{CompositeVerificationInput, verify_composite_with_gpu_appraiser};

const CASES: [&str; 5] = [
    "composite_positive",
    "composite_gpu_reject",
    "composite_cpu_reject_tampered_binding",
    "composite_pin_mismatch",
    "composite_gpu_prerequisite_reject",
];

const PYTHON_ORACLE: &str = r#"
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, os.environ["SOLSTONE_REPO_ROOT"])

from solstone.think.models import AttestationFailedError
from solstone.think.services.spp_attest.composite import verify_composite
from solstone.think.services.spp_attest.nvgpu.claims import (
    NvattestAcceptance,
    NvattestRejection,
    build_gpu_appraisal,
    classify_nvattest_result,
    parse_nvattest_stdout,
)
from solstone.think.services.spp_attest.nvgpu.errors import GpuAppraisalError
from solstone.think.services.spp_attest.snp import Policy, load_cpu_bundle


def owner_nonce(root):
    return bytes.fromhex("".join((root / "nonce.hex").read_text(encoding="utf-8").split()))


def failure_reason(exc):
    detail = exc.detail
    return detail.rsplit("(", 1)[1][:-1]


def evaluate(case):
    kind = case["kind"]
    root = Path(case["root"])
    gpu_calls = []

    def gpu_appraiser(envelope, nonce, *, nvattest_dir):
        gpu_calls.append(True)
        assert nonce == owner_nonce(root)
        assert nvattest_dir == root
        if kind == "composite_gpu_prerequisite_reject":
            raise GpuAppraisalError("nvattest_unavailable")
        stdout_name = "negA.stdout" if kind == "composite_gpu_reject" else "positive.stdout"
        stdout = parse_nvattest_stdout(
            (root / "nvattest" / stdout_name).read_text(encoding="utf-8")
        )
        verdict = classify_nvattest_result(0, stdout, owner_nonce=nonce)
        if isinstance(verdict, NvattestRejection):
            raise GpuAppraisalError(verdict.reason)
        assert isinstance(verdict, NvattestAcceptance)
        return build_gpu_appraisal(claim=verdict.claim, envelope=envelope, steps=[])

    policy = None
    if kind == "composite_pin_mismatch":
        policy = Policy(pcr_mode="pin", pcr_pins={"00" * 32})

    try:
        verdict = verify_composite(
            load_cpu_bundle(root),
            envelope_tlv=(root / "gpu-envelope.tlv").read_bytes(),
            channel_binding=(root / "guest_x25519.pub.der").read_bytes(),
            owner_nonce=owner_nonce(root),
            now=datetime.now(timezone.utc),
            nvattest_dir=root,
            policy=policy,
            gpu_appraiser=gpu_appraiser,
        )
        return {
            "case": kind,
            "status": "accepted",
            "verified": verdict.verified,
            "legs": list(verdict.legs),
            "substrate": verdict.substrate,
            "gpu_called": bool(gpu_calls),
        }
    except AttestationFailedError as exc:
        return {
            "case": kind,
            "status": "rejected",
            "reason": failure_reason(exc),
            "gpu_called": bool(gpu_calls),
        }


print(json.dumps(evaluate(json.loads(sys.argv[1])), sort_keys=True))
"#;

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

fn python() -> PathBuf {
    let python = repository_root().join(".venv/bin/python3");
    assert!(python.is_file(), "differential requires make install");
    python
}

fn python_verdict(kind: &str, root: &Path) -> Value {
    let output = Command::new(python())
        .arg("-c")
        .arg(PYTHON_ORACLE)
        .arg(json!({"kind": kind, "root": root}).to_string())
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python composite oracle");
    assert!(
        output.status.success(),
        "Python stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python verdict JSON")
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

#[test]
fn composite_matches_python_oracle() {
    for case in CASES {
        let fixture = TempFixture::copy_from(&fixture_root(), case);
        mutate_fixture(case, &fixture.path);
        let rust = rust_verdict(case, &fixture.path);
        let python = python_verdict(case, &fixture.path);
        assert_eq!(rust, python, "composite differential case {case}");
    }
}
