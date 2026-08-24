// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Production composite verifier and RA-TLS channel construction.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use ring::rand::{SecureRandom, SystemRandom};
use solstone_core_spp_attest::{
    CpuBundle, GpuAppraiser, NvattestGpuAppraiser, appraise_cpu_leg,
    error::{CpuLegError, GpuAppraisalReason, PcrFingerprintError},
    locate_nvattest, production_policy,
    tlv::decode_gpu_envelope,
};

use crate::{
    CompositeVerdict, CompositeVerificationError, NvattestEnsureStatus, RatlsChannelError,
    ratls::{
        channel::{AttestedChannel, RatlsEndpoint, establish_attested_channel},
        verify::{CompositeVerificationInput, CompositeVerifier},
    },
};

/// Production verifier backed by the locally provisioned nvattest payload.
pub struct ProductionCompositeVerifier {
    nvattest_dir: PathBuf,
    gpu_appraiser: Box<dyn GpuAppraiser + Send + Sync>,
}

impl ProductionCompositeVerifier {
    pub fn new(nvattest_dir: PathBuf) -> Self {
        Self {
            nvattest_dir,
            gpu_appraiser: Box::new(NvattestGpuAppraiser),
        }
    }
}

impl CompositeVerifier for ProductionCompositeVerifier {
    fn verify(
        &self,
        bundle: CpuBundle<'_>,
        input: CompositeVerificationInput<'_>,
    ) -> Result<CompositeVerdict, CompositeVerificationError> {
        verify_composite_with_gpu_appraiser(
            bundle,
            input,
            self.gpu_appraiser.as_ref(),
            &self.nvattest_dir,
            SystemTime::now(),
        )
    }
}

/// Verifies both attestation legs with an injectable GPU appraiser.
pub fn verify_composite_with_gpu_appraiser(
    bundle: CpuBundle<'_>,
    input: CompositeVerificationInput<'_>,
    gpu_appraiser: &dyn GpuAppraiser,
    nvattest_dir: &Path,
    now: SystemTime,
) -> Result<CompositeVerdict, CompositeVerificationError> {
    let cpu = appraise_cpu_leg(
        bundle,
        input.envelope_tlv,
        input.channel_binding,
        input.binding_domain,
        input.policy,
        input.quote_verifier,
    )
    .map_err(cpu_error)?;
    let envelope = decode_gpu_envelope(input.envelope_tlv)
        .map_err(|_| composite_error("cpu_verification_failed"))?;
    let owner_nonce: &[u8; 32] = input
        .owner_nonce
        .try_into()
        .map_err(|_| composite_error("gpu_appraisal_failed"))?;
    let gpu = gpu_appraiser
        .appraise(&envelope, owner_nonce, nvattest_dir)
        .map_err(gpu_error)?;

    Ok(CompositeVerdict {
        verified: true,
        legs: ["cpu", "gpu"],
        substrate: format!("AMD SEV-SNP + NVIDIA {}", gpu.hwmodel),
        checked_at: now,
        cpu,
        gpu,
    })
}

/// Checks whether the local nvattest payload is ready for an attestation attempt.
pub fn check_nvattest_readiness(nvattest_dir: &Path) -> NvattestEnsureStatus {
    match locate_nvattest(nvattest_dir) {
        Ok(_) => NvattestEnsureStatus::AlreadyInstalled,
        Err(GpuAppraisalReason::NvattestUnavailable) => NvattestEnsureStatus::Unavailable,
        Err(GpuAppraisalReason::NvattestIntegrityFailed) => NvattestEnsureStatus::IntegrityFailed,
        Err(GpuAppraisalReason::GpuNonceMismatch | GpuAppraisalReason::GpuAppraisalFailed) => {
            NvattestEnsureStatus::InstallFailed
        }
    }
}

/// Establishes one production-policy RA-TLS channel with a fresh owner nonce.
pub fn establish_production_attested_channel(
    endpoint: &RatlsEndpoint,
    nvattest_dir: &Path,
    socket_timeout: Duration,
) -> Result<AttestedChannel, RatlsChannelError> {
    let mut owner_nonce = [0u8; 32];
    SystemRandom::new()
        .fill(&mut owner_nonce)
        .map_err(|_| RatlsChannelError {
            reason_code: "nonce_generation_failed",
        })?;
    let policy = production_policy();
    let verifier = ProductionCompositeVerifier::new(nvattest_dir.to_path_buf());
    establish_attested_channel(
        endpoint,
        &owner_nonce,
        nvattest_dir,
        SystemTime::now(),
        None,
        Some(&policy),
        None,
        &verifier,
        socket_timeout,
        0,
    )
}

fn cpu_error(error: CpuLegError) -> CompositeVerificationError {
    match error {
        CpuLegError::PcrFingerprint {
            source: PcrFingerprintError::PinMismatch(_),
            ..
        } => composite_error("pcr_pin_mismatch"),
        _ => composite_error("cpu_verification_failed"),
    }
}

fn gpu_error(error: GpuAppraisalReason) -> CompositeVerificationError {
    composite_error(match error {
        GpuAppraisalReason::NvattestUnavailable => "nvattest_unavailable",
        GpuAppraisalReason::NvattestIntegrityFailed => "nvattest_integrity_failed",
        GpuAppraisalReason::GpuNonceMismatch => "gpu_nonce_mismatch",
        GpuAppraisalReason::GpuAppraisalFailed => "gpu_appraisal_failed",
    })
}

fn composite_error(reason_code: &'static str) -> CompositeVerificationError {
    CompositeVerificationError { reason_code }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::Path,
        sync::atomic::{AtomicBool, Ordering},
        time::SystemTime,
    };

    use solstone_core_spp_attest::{
        PcrMode,
        nvgpu::GpuAppraisal,
        snp::{AppraisalStep, TcbFloor},
    };

    use super::{check_nvattest_readiness, verify_composite_with_gpu_appraiser};
    use crate::{
        CompositeVerificationInput, NvattestEnsureStatus, classify_nvattest_prerequisite,
        test_support::TempDir,
    };

    struct FixtureGpuAppraiser {
        result: Result<GpuAppraisal, solstone_core_spp_attest::error::GpuAppraisalReason>,
        called: AtomicBool,
    }

    impl FixtureGpuAppraiser {
        fn accepted() -> Self {
            Self {
                result: Ok(GpuAppraisal {
                    steps: vec![AppraisalStep {
                        name: "nvattest",
                        status: "ok",
                        detail: String::new(),
                    }],
                    driver_version: String::new(),
                    vbios_version: String::new(),
                    hwmodel: "attested-hwmodel".to_owned(),
                    ueid: String::new(),
                    oemid: String::new(),
                    eat_nonce: String::new(),
                    claims_version: String::new(),
                    arch: "UNTRUSTED-ENVELOPE-ARCH".to_owned(),
                    envelope_gpu_uuid: String::new(),
                }),
                called: AtomicBool::new(false),
            }
        }

        fn rejected(reason: solstone_core_spp_attest::error::GpuAppraisalReason) -> Self {
            Self {
                result: Err(reason),
                called: AtomicBool::new(false),
            }
        }
    }

    impl solstone_core_spp_attest::GpuAppraiser for FixtureGpuAppraiser {
        fn appraise(
            &self,
            _: &solstone_core_spp_attest::tlv::GpuEnvelope,
            _: &[u8; 32],
            _: &Path,
        ) -> Result<GpuAppraisal, solstone_core_spp_attest::error::GpuAppraisalReason> {
            self.called.store(true, Ordering::SeqCst);
            self.result.clone()
        }
    }

    struct Fixture {
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

    impl Fixture {
        fn load() -> Self {
            let bytes = |name: &str| {
                let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .ancestors()
                    .nth(3)
                    .expect("repository root")
                    .join("tests/fixtures/spp_attest");
                std::fs::read(root.join(name)).expect("read fixture")
            };
            let nonce_hex = String::from_utf8(bytes("nonce.hex")).expect("nonce UTF-8");
            let nonce = nonce_hex
                .split_whitespace()
                .collect::<String>()
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("byte")
                })
                .collect::<Vec<_>>()
                .try_into()
                .expect("32-byte nonce");
            Self {
                nonce,
                hcl_report: bytes("hcl_report.bin"),
                report: bytes("report.bin"),
                ark: bytes("certs/ark.pem"),
                ask: bytes("certs/ask.pem"),
                vcek: bytes("certs/vcek.pem"),
                ak: bytes("akpub.pem"),
                quote_message: bytes("quote.msg"),
                quote_signature: bytes("quote.sig"),
                quote_pcrs: bytes("quote.pcrs"),
                envelope: bytes("gpu-envelope.tlv"),
                channel_binding: bytes("guest_x25519.pub.der"),
            }
        }

        fn verify(
            &self,
            policy: Option<&solstone_core_spp_attest::Policy>,
            appraiser: &dyn solstone_core_spp_attest::GpuAppraiser,
        ) -> Result<crate::CompositeVerdict, crate::CompositeVerificationError> {
            let certificate_chain = [&self.ark[..], &self.ask[..], &self.vcek[..]];
            verify_composite_with_gpu_appraiser(
                solstone_core_spp_attest::CpuBundle {
                    hcl_report: &self.hcl_report,
                    standalone_report: Some(&self.report),
                    cert_pems: &certificate_chain,
                    ak_public_key_pem: &self.ak,
                    nonce: &self.nonce,
                    quote_message: &self.quote_message,
                    quote_signature: &self.quote_signature,
                    quote_pcrs: &self.quote_pcrs,
                },
                CompositeVerificationInput {
                    envelope_tlv: &self.envelope,
                    channel_binding: &self.channel_binding,
                    owner_nonce: &self.nonce,
                    now: SystemTime::UNIX_EPOCH,
                    nvattest_dir: Path::new("unused"),
                    binding_domain: solstone_core_spp_attest::binding::BINDING_DOMAIN,
                    roots_dir: None,
                    policy,
                    quote_verifier: None,
                },
                appraiser,
                Path::new("unused"),
                SystemTime::UNIX_EPOCH,
            )
        }
    }

    #[test]
    fn composite_positive_uses_attested_gpu_hwmodel_for_substrate() {
        let fixture = Fixture::load();
        let appraiser = FixtureGpuAppraiser::accepted();
        let policy = solstone_core_spp_attest::production_policy();

        let verdict = fixture
            .verify(Some(&policy), &appraiser)
            .expect("fixture composite verifies");
        assert!(verdict.verified);
        assert_eq!(verdict.legs, ["cpu", "gpu"]);
        assert_eq!(verdict.substrate, "AMD SEV-SNP + NVIDIA attested-hwmodel");
        assert!(appraiser.called.load(Ordering::SeqCst));
    }

    #[test]
    fn pin_mode_rejects_before_gpu_and_record_mode_accepts_the_same_fixture() {
        let fixture = Fixture::load();
        let appraiser = FixtureGpuAppraiser::accepted();
        let rejected_policy = solstone_core_spp_attest::Policy {
            pcr_mode: PcrMode::Pin,
            pcr_pins: ["00".repeat(32)].into_iter().collect(),
            ..solstone_core_spp_attest::Policy::default()
        };
        let error = fixture
            .verify(Some(&rejected_policy), &appraiser)
            .expect_err("mismatched pin rejects");
        assert_eq!(error.reason_code, "pcr_pin_mismatch");
        assert!(!appraiser.called.load(Ordering::SeqCst));
        assert!(!error.to_string().contains(&"00".repeat(32)));
        assert!(
            !error
                .to_string()
                .contains("b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3")
        );

        let appraiser = FixtureGpuAppraiser::accepted();
        let record_policy = solstone_core_spp_attest::Policy {
            pcr_mode: PcrMode::Record,
            ..solstone_core_spp_attest::Policy::default()
        };
        assert!(fixture.verify(Some(&record_policy), &appraiser).is_ok());
    }

    #[test]
    fn empty_pin_mode_is_a_hard_composite_error() {
        let fixture = Fixture::load();
        let appraiser = FixtureGpuAppraiser::accepted();
        let policy = solstone_core_spp_attest::Policy {
            pcr_mode: PcrMode::Pin,
            ..solstone_core_spp_attest::Policy::default()
        };

        assert_eq!(
            fixture.verify(Some(&policy), &appraiser),
            Err(crate::CompositeVerificationError {
                reason_code: "pcr_pin_mismatch"
            })
        );
        assert!(!appraiser.called.load(Ordering::SeqCst));
    }

    #[test]
    fn gpu_failure_is_required_and_preserves_its_closed_reason() {
        let fixture = Fixture::load();
        let appraiser = FixtureGpuAppraiser::rejected(
            solstone_core_spp_attest::error::GpuAppraisalReason::GpuNonceMismatch,
        );

        assert_eq!(
            fixture.verify(
                Some(&solstone_core_spp_attest::production_policy()),
                &appraiser
            ),
            Err(crate::CompositeVerificationError {
                reason_code: "gpu_nonce_mismatch"
            })
        );
        assert!(appraiser.called.load(Ordering::SeqCst));
    }

    #[test]
    fn cpu_failure_does_not_call_gpu_or_leak_fixture_material() {
        let fixture = Fixture::load();
        let appraiser = FixtureGpuAppraiser::accepted();
        let mut tampered = fixture;
        tampered.channel_binding = b"tampered-binding".to_vec();
        let error = tampered
            .verify(
                Some(&solstone_core_spp_attest::production_policy()),
                &appraiser,
            )
            .expect_err("tampered CPU binding rejects");

        assert_eq!(error.reason_code, "cpu_verification_failed");
        assert!(!appraiser.called.load(Ordering::SeqCst));
        let rendered = error.to_string();
        assert!(!rendered.contains("BEGIN PUBLIC KEY"));
        assert!(!rendered.contains("tampered-binding"));
    }

    #[test]
    fn cpu_failure_on_envelope_nonce_mismatch_does_not_call_gpu() {
        let fixture = Fixture::load();
        let appraiser = FixtureGpuAppraiser::accepted();
        let mut tampered = fixture;
        tampered.envelope[16] ^= 1;
        let error = tampered
            .verify(
                Some(&solstone_core_spp_attest::production_policy()),
                &appraiser,
            )
            .expect_err("envelope nonce mismatch rejects");

        assert_eq!(error.reason_code, "cpu_verification_failed");
        assert!(!appraiser.called.load(Ordering::SeqCst));
        let rendered = error.to_string();
        let nonce_hex = tampered
            .nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(!rendered.contains(&nonce_hex));
        assert!(!rendered.contains("SPPGPU1"));
    }

    #[test]
    fn gpu_prerequisite_reasons_reject_without_cpu_only_fallback() {
        let fixture = Fixture::load();
        for (reason, expected) in [
            (
                solstone_core_spp_attest::error::GpuAppraisalReason::NvattestUnavailable,
                "nvattest_unavailable",
            ),
            (
                solstone_core_spp_attest::error::GpuAppraisalReason::NvattestIntegrityFailed,
                "nvattest_integrity_failed",
            ),
        ] {
            let appraiser = FixtureGpuAppraiser::rejected(reason);
            assert_eq!(
                fixture.verify(
                    Some(&solstone_core_spp_attest::production_policy()),
                    &appraiser
                ),
                Err(crate::CompositeVerificationError {
                    reason_code: expected
                })
            );
        }
    }

    #[test]
    fn composite_enforces_hcla_report_vmpl_and_tcb_policy_fields_before_gpu() {
        let fixture = Fixture::load();
        let tcb_policy = solstone_core_spp_attest::Policy {
            min_tcb: BTreeMap::from([(
                "current".to_owned(),
                TcbFloor {
                    boot_loader: Some(11),
                    ..TcbFloor::default()
                },
            )]),
            ..solstone_core_spp_attest::Policy::default()
        };
        let policies = [
            solstone_core_spp_attest::Policy {
                allowed_hcla_versions: Default::default(),
                ..solstone_core_spp_attest::Policy::default()
            },
            solstone_core_spp_attest::Policy {
                allowed_report_versions: [4].into_iter().collect(),
                ..solstone_core_spp_attest::Policy::default()
            },
            solstone_core_spp_attest::Policy {
                allowed_vmpl: [1].into_iter().collect(),
                ..solstone_core_spp_attest::Policy::default()
            },
            tcb_policy,
        ];

        for policy in policies {
            let appraiser = FixtureGpuAppraiser::accepted();
            assert_eq!(
                fixture.verify(Some(&policy), &appraiser),
                Err(crate::CompositeVerificationError {
                    reason_code: "cpu_verification_failed"
                })
            );
            assert!(!appraiser.called.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn readiness_preserves_each_locator_cause() {
        let root = TempDir::new("readiness");
        assert_eq!(
            check_nvattest_readiness(&root.path().join("missing")),
            NvattestEnsureStatus::Unavailable
        );
        assert_eq!(
            check_nvattest_readiness(root.path()),
            NvattestEnsureStatus::Unavailable
        );

        fs::create_dir_all(root.path().join("bin")).expect("create binary directory");
        fs::create_dir_all(root.path().join("lib")).expect("create library directory");
        fs::write(root.path().join("bin/nvattest"), "placeholder").expect("write binary");
        assert_eq!(
            check_nvattest_readiness(root.path()),
            NvattestEnsureStatus::IntegrityFailed
        );

        fs::create_dir_all(root.path().join("share/ca")).expect("create CA directory");
        fs::write(root.path().join("share/ca/ca-bundle.pem"), "CA").expect("write CA bundle");
        assert_eq!(
            check_nvattest_readiness(root.path()),
            NvattestEnsureStatus::AlreadyInstalled
        );
        assert_eq!(
            classify_nvattest_prerequisite(NvattestEnsureStatus::AlreadyInstalled),
            None
        );
    }
}
