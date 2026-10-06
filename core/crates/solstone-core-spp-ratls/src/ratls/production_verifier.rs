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
    error::{CpuLegError, GpuAppraisalReason, PcrFingerprintError, SnpVerifyError},
    locate_nvattest,
    nvgpu::{GpuProfiles, GpuStatusInput},
    production_policy,
    snp::CpuAppraisal,
    tlv::decode_gpu_envelope,
};

use crate::{
    CompositeVerdict, CompositeVerificationError, NvattestEnsureStatus, RatlsChannelError,
    ratls::{
        channel::{
            AdmissionClock, AttestedChannel, RatlsEndpoint, SystemAdmissionClock,
            establish_attested_channel_with_clock,
        },
        verify::{CompositeVerificationInput, CompositeVerifier},
    },
};

/// Production verifier backed by the locally provisioned nvattest payload.
pub struct ProductionCompositeVerifier {
    nvattest_dir: PathBuf,
    gpu_appraiser: Box<dyn GpuAppraiser + Send + Sync>,
    profiles: GpuProfiles,
}

impl ProductionCompositeVerifier {
    pub fn new(nvattest_dir: PathBuf) -> Self {
        Self::with_profiles(nvattest_dir, GpuProfiles::production())
    }

    /// A verifier whose GPU profiles are supplied by the caller, for tests and
    /// instruments that also inject their own CPU policy.
    pub fn with_profiles(nvattest_dir: PathBuf, profiles: GpuProfiles) -> Self {
        Self {
            nvattest_dir,
            gpu_appraiser: Box::new(NvattestGpuAppraiser),
            profiles,
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
            &self.profiles,
            &self.nvattest_dir,
            SystemTime::now(),
        )
    }
}

/// Verifies both attestation legs with an injectable GPU appraiser.
///
/// The CPU leg runs first. Only its verified fingerprint selects the GPU
/// profile, and an unknown fingerprint stops before any GPU work.
pub fn verify_composite_with_gpu_appraiser(
    bundle: CpuBundle<'_>,
    input: CompositeVerificationInput<'_>,
    gpu_appraiser: &dyn GpuAppraiser,
    profiles: &GpuProfiles,
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
    verify_gpu_after_cpu(cpu, &input, gpu_appraiser, profiles, nvattest_dir, now)
}

/// The GPU half of composite verification, after a verified CPU leg.
///
/// `now` is this device's clock, the only time offline status proofs are
/// judged against. Exposed so an instrument that cannot reproduce a CPU leg
/// (a synthetic gateway) still drives the real profile selection and helper.
pub fn verify_gpu_after_cpu(
    cpu: CpuAppraisal,
    input: &CompositeVerificationInput<'_>,
    gpu_appraiser: &dyn GpuAppraiser,
    profiles: &GpuProfiles,
    nvattest_dir: &Path,
    now: SystemTime,
) -> Result<CompositeVerdict, CompositeVerificationError> {
    let profile = profiles.select(&cpu.pcr_sha256).map_err(gpu_error)?;
    let envelope = decode_gpu_envelope(input.envelope_tlv)
        .map_err(|_| composite_error("cpu_verification_failed"))?;
    let owner_nonce: &[u8; 32] = input
        .owner_nonce
        .try_into()
        .map_err(|_| composite_error("gpu_appraisal_failed"))?;
    let gpu = gpu_appraiser
        .appraise(
            &envelope,
            owner_nonce,
            nvattest_dir,
            &GpuStatusInput {
                profile,
                proofs: input.status_proofs,
                verification_time: now,
            },
        )
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
        Err(
            GpuAppraisalReason::GpuNonceMismatch
            | GpuAppraisalReason::GpuAppraisalFailed
            | GpuAppraisalReason::StatusProfileMissing
            | GpuAppraisalReason::StatusProofsMissing,
        ) => NvattestEnsureStatus::InstallFailed,
    }
}

/// Establishes one production-policy RA-TLS channel with a fresh owner nonce.
pub fn establish_production_attested_channel(
    endpoint: &RatlsEndpoint,
    nvattest_dir: &Path,
    socket_timeout: Duration,
    epoch: u64,
) -> Result<AttestedChannel, RatlsChannelError> {
    establish_production_attested_channel_with_clock(
        endpoint,
        nvattest_dir,
        socket_timeout,
        epoch,
        &SystemAdmissionClock,
    )
}

/// [`establish_production_attested_channel`], admitted on the caller's clock,
/// so a pool measures channel age on the same clock it admitted with.
pub fn establish_production_attested_channel_with_clock(
    endpoint: &RatlsEndpoint,
    nvattest_dir: &Path,
    socket_timeout: Duration,
    epoch: u64,
    clock: &dyn AdmissionClock,
) -> Result<AttestedChannel, RatlsChannelError> {
    let mut owner_nonce = [0u8; 32];
    SystemRandom::new()
        .fill(&mut owner_nonce)
        .map_err(|_| RatlsChannelError {
            reason_code: "nonce_generation_failed",
        })?;
    let policy = production_policy();
    let verifier = ProductionCompositeVerifier::new(nvattest_dir.to_path_buf());
    establish_attested_channel_with_clock(
        endpoint,
        &owner_nonce,
        nvattest_dir,
        SystemTime::now(),
        None,
        Some(&policy),
        None,
        &verifier,
        socket_timeout,
        epoch,
        clock,
    )
}

fn cpu_error(error: CpuLegError) -> CompositeVerificationError {
    match error {
        CpuLegError::PcrFingerprint {
            source: PcrFingerprintError::PinMismatch(_),
            ..
        } => composite_error("pcr_pin_mismatch"),
        CpuLegError::SnpVerify {
            source: SnpVerifyError::PolicyIdKeyAbsent | SnpVerifyError::PolicyIdKeyNotPinned,
            ..
        } => composite_error("id_key_pin_mismatch"),
        _ => composite_error("cpu_verification_failed"),
    }
}

fn gpu_error(error: GpuAppraisalReason) -> CompositeVerificationError {
    composite_error(match error {
        GpuAppraisalReason::NvattestUnavailable => "nvattest_unavailable",
        GpuAppraisalReason::NvattestIntegrityFailed => "nvattest_integrity_failed",
        GpuAppraisalReason::GpuNonceMismatch => "gpu_nonce_mismatch",
        GpuAppraisalReason::GpuAppraisalFailed => "gpu_appraisal_failed",
        GpuAppraisalReason::StatusProfileMissing => "gpu_status_profile_missing",
        GpuAppraisalReason::StatusProofsMissing => "gpu_status_proofs_missing",
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

    use solstone_core_spp_attest::nvgpu::{
        GpuProfile, GpuProfiles, GpuStatusInput, ManifestSet, StatusMode,
    };

    use super::{check_nvattest_readiness, verify_composite_with_gpu_appraiser};

    const CURRENT_PIN: &str = "b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3";

    fn profiles(entries: &[(&str, StatusMode)]) -> GpuProfiles {
        GpuProfiles::from_profiles(
            entries
                .iter()
                .map(|(pin, mode)| GpuProfile::new(*pin, ManifestSet::QUALIFIED_595_71_05, *mode))
                .collect(),
        )
    }

    #[test]
    fn the_verified_cpu_fingerprint_alone_selects_the_status_mode() {
        let fixture = Fixture::load();
        let policy = solstone_core_spp_attest::production_policy();
        let successor = "44".repeat(32);
        // Test-only coexistence: the current pin online, a successor offline.
        let both = profiles(&[
            (CURRENT_PIN, StatusMode::OnlineNonce),
            (&successor, StatusMode::OfflineSignedAge),
        ]);
        for proofs in [None, Some(&b"proofs from the engine"[..])] {
            let appraiser = FixtureGpuAppraiser::accepted();
            fixture
                .verify_with(Some(&policy), &appraiser, &both, proofs)
                .expect("current image verifies online");
            let (mode, seen_proofs, _) = appraiser.seen.lock().unwrap().clone().unwrap();
            // Proof presence never selects or changes the mode.
            assert_eq!(mode, StatusMode::OnlineNonce);
            assert_eq!(seen_proofs.as_deref(), proofs);
        }

        // The same CPU evidence under a profile set that maps it offline is
        // judged offline, with this device's time and the engine's proofs.
        let offline = profiles(&[(CURRENT_PIN, StatusMode::OfflineSignedAge)]);
        let appraiser = FixtureGpuAppraiser::accepted();
        fixture
            .verify_with(Some(&policy), &appraiser, &offline, Some(b"proofs"))
            .expect("offline stub verifies");
        let (mode, seen_proofs, verification_time) =
            appraiser.seen.lock().unwrap().clone().unwrap();
        assert_eq!(mode, StatusMode::OfflineSignedAge);
        assert_eq!(seen_proofs.as_deref(), Some(&b"proofs"[..]));
        assert_eq!(
            verification_time,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_996_648)
        );
    }

    #[test]
    fn an_admitted_pin_without_a_profile_stops_before_gpu_work() {
        let fixture = Fixture::load();
        let policy = solstone_core_spp_attest::production_policy();
        for set in [
            profiles(&[]),
            profiles(&[(&"55".repeat(32), StatusMode::OfflineSignedAge)]),
        ] {
            let appraiser = FixtureGpuAppraiser::accepted();
            assert_eq!(
                fixture.verify_with(Some(&policy), &appraiser, &set, Some(b"proofs")),
                Err(crate::CompositeVerificationError {
                    reason_code: "gpu_status_profile_missing"
                })
            );
            assert!(!appraiser.called.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn production_profiles_cover_exactly_the_production_pins() {
        let profiles = GpuProfiles::production();
        let expected = [
            (CURRENT_PIN, StatusMode::OnlineNonce),
            (
                "78d2cb684535a82591ef69490542ee9f1501b463675523ac76c834413e616180",
                StatusMode::OfflineSignedAge,
            ),
            (
                "96e66fc57838c29daa2b2f3b9301f5aeed3a9285fea2b4439053a7c7c7908952",
                StatusMode::OfflineSignedAge,
            ),
        ];
        assert_eq!(
            solstone_core_spp_attest::PRODUCTION_PCR_SHA256_PINS.len(),
            expected.len()
        );
        for (pin, status) in expected {
            assert!(solstone_core_spp_attest::PRODUCTION_PCR_SHA256_PINS.contains(&pin));
            assert_eq!(profiles.select(pin).expect("profile").status(), status);
        }
    }
    use crate::{
        CompositeVerificationInput, NvattestEnsureStatus, classify_nvattest_prerequisite,
        test_support::TempDir,
    };

    type SeenStatus = (StatusMode, Option<Vec<u8>>, SystemTime);

    struct FixtureGpuAppraiser {
        result: Result<GpuAppraisal, solstone_core_spp_attest::error::GpuAppraisalReason>,
        called: AtomicBool,
        seen: std::sync::Mutex<Option<SeenStatus>>,
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
                    status: solstone_core_spp_attest::nvgpu::GpuStatusAuthorization::OnlineNonce,
                }),
                called: AtomicBool::new(false),
                seen: std::sync::Mutex::new(None),
            }
        }

        fn rejected(reason: solstone_core_spp_attest::error::GpuAppraisalReason) -> Self {
            Self {
                result: Err(reason),
                called: AtomicBool::new(false),
                seen: std::sync::Mutex::new(None),
            }
        }
    }

    impl solstone_core_spp_attest::GpuAppraiser for FixtureGpuAppraiser {
        fn appraise(
            &self,
            _: &solstone_core_spp_attest::tlv::GpuEnvelope,
            _: &[u8; 32],
            _: &Path,
            status: &GpuStatusInput<'_>,
        ) -> Result<GpuAppraisal, solstone_core_spp_attest::error::GpuAppraisalReason> {
            self.called.store(true, Ordering::SeqCst);
            *self.seen.lock().unwrap() = Some((
                status.profile.status(),
                status.proofs.map(<[u8]>::to_vec),
                status.verification_time,
            ));
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
            self.verify_with(policy, appraiser, &GpuProfiles::production(), None)
        }

        fn verify_with(
            &self,
            policy: Option<&solstone_core_spp_attest::Policy>,
            appraiser: &dyn solstone_core_spp_attest::GpuAppraiser,
            profiles: &GpuProfiles,
            status_proofs: Option<&[u8]>,
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
                    status_proofs,
                },
                appraiser,
                profiles,
                Path::new("unused"),
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_996_648),
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
    fn id_key_pin_rejects_before_gpu_with_its_own_reason() {
        let fixture = Fixture::load();
        for pinned in [
            std::collections::BTreeSet::from([[0x5a; 48]]),
            std::collections::BTreeSet::new(),
        ] {
            let appraiser = FixtureGpuAppraiser::accepted();
            let policy = solstone_core_spp_attest::Policy {
                id_key_digests: Some(pinned),
                ..solstone_core_spp_attest::production_policy()
            };
            let error = fixture
                .verify(Some(&policy), &appraiser)
                .expect_err("unpinned firmware signer rejects");
            assert_eq!(error.reason_code, "id_key_pin_mismatch");
            assert!(!appraiser.called.load(Ordering::SeqCst));
            assert!(!error.to_string().contains("942fd93e"));
        }
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
