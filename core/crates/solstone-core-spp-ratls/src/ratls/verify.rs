// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::SystemTime;

use solstone_core_spp_attest::{
    CpuBundle, Policy, QuoteVerifier, check_pcr_fingerprint,
    error::PcrFingerprintError,
    tpm_quote::{TpmQuoteInput, verify_quote},
};
use x509_parser::{
    extensions::ParsedExtension,
    prelude::{FromDer, X509Certificate},
};

use crate::{
    cadence::CompositeVerdict,
    error::{CompositeVerificationError, RatlsVerificationError},
    ratls::contract::{
        CERTIFICATE_BINDING_DOMAIN, COMPOSITE_EVIDENCE_OID, CompositeEvidence, ExporterProof,
        exporter_binding,
    },
};

pub struct CompositeVerificationInput<'a> {
    pub envelope_tlv: &'a [u8],
    pub channel_binding: &'a [u8],
    pub owner_nonce: &'a [u8],
    pub now: SystemTime,
    pub nvattest_dir: &'a Path,
    pub binding_domain: &'a [u8],
    pub roots_dir: Option<&'a Path>,
    pub policy: Option<&'a Policy>,
    pub quote_verifier: Option<&'a dyn QuoteVerifier>,
}

pub trait CompositeVerifier: Send + Sync {
    fn verify(
        &self,
        bundle: CpuBundle<'_>,
        input: CompositeVerificationInput<'_>,
    ) -> Result<CompositeVerdict, CompositeVerificationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCertificateEvidence {
    pub evidence: CompositeEvidence,
    pub verdict: CompositeVerdict,
    pub tls_spki_der: Vec<u8>,
}

#[allow(clippy::too_many_arguments)] // Mirrors the injectable Python verifier seam.
pub fn verify_certificate_evidence(
    certificate_der: &[u8],
    owner_nonce: &[u8],
    now: SystemTime,
    nvattest_dir: &Path,
    roots_dir: Option<&Path>,
    policy: Option<&Policy>,
    quote_verifier: Option<&dyn QuoteVerifier>,
    composite_verifier: &dyn CompositeVerifier,
) -> Result<VerifiedCertificateEvidence, RatlsVerificationError> {
    let (remaining, certificate) =
        X509Certificate::from_der(certificate_der).map_err(|_| RatlsVerificationError {
            reason_code: "certificate_invalid",
        })?;
    if !remaining.is_empty() {
        return Err(RatlsVerificationError {
            reason_code: "certificate_invalid",
        });
    }
    let extension = certificate
        .extensions()
        .iter()
        .find(|extension| extension.oid.to_id_string() == COMPOSITE_EVIDENCE_OID)
        .ok_or(RatlsVerificationError {
            reason_code: "certificate_extension_missing",
        })?;
    if !extension.critical {
        return Err(RatlsVerificationError {
            reason_code: "certificate_extension_not_critical",
        });
    }
    if !matches!(
        extension.parsed_extension(),
        ParsedExtension::UnsupportedExtension { .. }
    ) {
        return Err(RatlsVerificationError {
            reason_code: "certificate_extension_invalid",
        });
    }
    let evidence =
        CompositeEvidence::from_der(extension.value).map_err(|_| RatlsVerificationError {
            reason_code: "certificate_evidence_invalid",
        })?;
    if evidence.owner_nonce != owner_nonce {
        return Err(RatlsVerificationError {
            reason_code: "nonce_mismatch",
        });
    }
    let tls_spki_der = certificate.public_key().raw.to_vec();
    if evidence.tls_spki_der != tls_spki_der {
        return Err(RatlsVerificationError {
            reason_code: "spki_mismatch",
        });
    }
    let channel_binding = ring::digest::digest(&ring::digest::SHA256, &tls_spki_der)
        .as_ref()
        .to_vec();
    let bundle = CpuBundle {
        hcl_report: &evidence.hcl_report,
        standalone_report: Some(&evidence.amd_report),
        cert_pems: &[
            &evidence.amd_ark_pem,
            &evidence.amd_ask_pem,
            &evidence.amd_vcek_pem,
        ],
        ak_public_key_pem: &evidence.ak_public_key_pem,
        nonce: &evidence.owner_nonce,
        quote_message: &evidence.quote_message,
        quote_signature: &evidence.quote_signature,
        quote_pcrs: &evidence.quote_pcrs,
    };
    let verdict = composite_verifier
        .verify(
            bundle,
            CompositeVerificationInput {
                envelope_tlv: &evidence.gpu_envelope,
                channel_binding: &channel_binding,
                owner_nonce,
                now,
                nvattest_dir,
                binding_domain: CERTIFICATE_BINDING_DOMAIN,
                roots_dir,
                policy,
                quote_verifier,
            },
        )
        .map_err(|error| RatlsVerificationError {
            reason_code: error.reason_code,
        })?;
    Ok(VerifiedCertificateEvidence {
        evidence,
        verdict,
        tls_spki_der,
    })
}

pub fn verify_exporter_proof(
    proof_der: &[u8],
    evidence: &CompositeEvidence,
    tls_exporter: &[u8],
    owner_nonce: &[u8],
    policy: Option<&Policy>,
) -> Result<(), RatlsVerificationError> {
    let proof = ExporterProof::from_der(proof_der).map_err(|_| RatlsVerificationError {
        reason_code: "exporter_proof_invalid",
    })?;
    if proof.owner_nonce != owner_nonce {
        return Err(RatlsVerificationError {
            reason_code: "nonce_mismatch",
        });
    }
    if proof.tls_spki_der != evidence.tls_spki_der {
        return Err(RatlsVerificationError {
            reason_code: "spki_mismatch",
        });
    }
    if proof.tls_exporter != tls_exporter {
        return Err(RatlsVerificationError {
            reason_code: "exporter_mismatch",
        });
    }
    let expected_binding = exporter_binding(
        owner_nonce,
        &evidence.tls_spki_der,
        tls_exporter,
        &evidence.gpu_envelope,
    );
    verify_quote(TpmQuoteInput {
        ak_public_key_pem: &evidence.ak_public_key_pem,
        quote_msg: &proof.quote_message,
        quote_sig: &proof.quote_signature,
        quote_pcrs: &proof.quote_pcrs,
        expected_binding: &expected_binding,
    })
    .map_err(|_| RatlsVerificationError {
        reason_code: "exporter_quote_failed",
    })?;
    match check_pcr_fingerprint(&proof.quote_pcrs, policy.unwrap_or(&Policy::default())) {
        Ok(_) => Ok(()),
        Err(PcrFingerprintError::PinMismatch(_)) => Err(RatlsVerificationError {
            reason_code: "pcr_pin_mismatch",
        }),
        Err(_) => Err(RatlsVerificationError {
            reason_code: "composite_appraisal_failed",
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use rcgen::{CertificateParams, CustomExtension, KeyPair, PKCS_ECDSA_P256_SHA256};
    use x509_parser::prelude::FromDer;

    use super::*;

    struct RejectingCompositeVerifier;

    impl CompositeVerifier for RejectingCompositeVerifier {
        fn verify(
            &self,
            _: CpuBundle<'_>,
            _: CompositeVerificationInput<'_>,
        ) -> Result<CompositeVerdict, CompositeVerificationError> {
            Err(CompositeVerificationError {
                reason_code: "composite_appraisal_failed",
            })
        }
    }

    #[test]
    fn certificate_evidence_carries_the_injected_composite_seam() {
        let owner_nonce = [7; 32];
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
        let params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
        let base_certificate = params.self_signed(&key).expect("base certificate");
        let (_, parsed) = X509Certificate::from_der(base_certificate.der()).expect("parse cert");
        let evidence = CompositeEvidence {
            owner_nonce: owner_nonce.to_vec(),
            tls_spki_der: parsed.public_key().raw.to_vec(),
            amd_report: Vec::new(),
            hcl_report: Vec::new(),
            ak_public_key_pem: Vec::new(),
            quote_message: Vec::new(),
            quote_signature: Vec::new(),
            quote_pcrs: Vec::new(),
            amd_ark_pem: Vec::new(),
            amd_ask_pem: Vec::new(),
            amd_vcek_pem: Vec::new(),
            gpu_envelope: Vec::new(),
        };
        let mut params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
        let mut extension = CustomExtension::from_oid_content(
            &[
                2,
                25,
                3_708_997_813,
                3_535_365_757,
                2_172_800_616,
                1_077_671_698,
            ],
            evidence.to_der(),
        );
        extension.set_criticality(true);
        params.custom_extensions.push(extension);
        let certificate = params.self_signed(&key).expect("evidence certificate");
        let error = verify_certificate_evidence(
            certificate.der(),
            &owner_nonce,
            SystemTime::now(),
            Path::new("."),
            None,
            None,
            None,
            &RejectingCompositeVerifier,
        )
        .expect_err("injected composite rejection must fail the certificate");
        assert_eq!(error.reason_code, "composite_appraisal_failed");
    }
}
