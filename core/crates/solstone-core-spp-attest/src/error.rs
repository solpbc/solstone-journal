// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Fail-closed errors raised while decoding an SPP GPU TLV envelope.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TlvError {
    #[error("GPU envelope header is truncated")]
    TruncatedHeader,
    #[error("GPU envelope magic does not match")]
    MagicMismatch,
    #[error("GPU envelope field count does not match")]
    FieldCountMismatch,
    #[error("GPU envelope fields are out of order")]
    FieldOutOfOrder,
    #[error("GPU envelope field length overruns input")]
    FieldLengthOverrun,
    #[error("GPU envelope has trailing bytes")]
    TrailingBytes,
    #[error("GPU envelope contains an unknown field identifier")]
    UnknownFieldId,
    #[error("GPU envelope contains a duplicate field identifier")]
    DuplicateFieldId,
    #[error("GPU envelope is missing a required field identifier")]
    MissingFieldId,
    #[error("GPU envelope nonce has an invalid length")]
    NonceLength,
    #[error("SPDM report is too short for its nonce")]
    SpdmTooShort,
    #[error("SPDM report GET_MEASUREMENTS header does not match")]
    SpdmHeaderMismatch,
}

/// Fail-closed errors raised while binding CPU and GPU evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BindingError {
    #[error("binding nonce has an invalid length")]
    NonceLength,
    #[error("channel binding is empty")]
    ChannelBindingEmpty,
    #[error("GPU envelope is empty")]
    EnvelopeEmpty,
    #[error("binding domain is empty")]
    DomainEmpty,
    #[error("GPU envelope nonce does not match owner nonce")]
    EnvelopeNonceMismatch,
    #[error("SPDM nonce does not match GPU envelope nonce")]
    SpdmNonceMismatch,
}

/// Fail-closed errors raised while verifying TPM2 quote evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TpmQuoteError {
    #[error("AK public key PEM did not parse")]
    AkPemInvalid,
    #[error("AK public key is not RSA")]
    AkNotRsa,
    #[error("expected TPM quote binding has an invalid length")]
    ExpectedBindingLength,
    #[error("TPMS_ATTEST is truncated")]
    TruncatedAttest,
    #[error("TPMS_ATTEST has trailing bytes")]
    TrailingAttestBytes,
    #[error("TPMS_ATTEST magic does not match")]
    MagicMismatch,
    #[error("TPMS_ATTEST type is not a quote")]
    AttestationTypeMismatch,
    #[error("TPM quote extraData does not match the expected binding")]
    ExtraDataMismatch,
    #[error("TPM quote PCR selection count is unsupported")]
    PcrSelectionCount,
    #[error("TPM quote PCR hash algorithm is unsupported")]
    PcrHashAlgorithm,
    #[error("TPM quote PCR selection size is invalid")]
    PcrSelectionSize,
    #[error("TPM quote PCR selection is empty")]
    PcrSelectionEmpty,
    #[error("TPM quote PCR digest size is invalid")]
    PcrDigestSize,
    #[error("TPMT_SIGNATURE is truncated")]
    TruncatedSignature,
    #[error("TPMT_SIGNATURE has trailing bytes")]
    TrailingSignatureBytes,
    #[error("TPM signature algorithm is unsupported")]
    SignatureAlgorithm,
    #[error("TPM signature hash algorithm is unsupported")]
    SignatureHashAlgorithm,
    #[error("TPM signature length does not match the AK modulus")]
    SignatureLength,
    #[error("TPM quote signature is invalid")]
    SignatureInvalid,
    #[error("quote.pcrs selection count is invalid")]
    PcrFileSelectionCount,
    #[error("quote.pcrs contains a nonzero inactive selection slot")]
    PcrInactiveSlotNonZero,
    #[error("quote.pcrs selection padding is nonzero")]
    PcrSelectionPaddingNonZero,
    #[error("quote.pcrs digest list count is invalid")]
    PcrDigestListCount,
    #[error("quote.pcrs digest slot is invalid")]
    PcrDigestSlotInvalid,
    #[error("quote.pcrs selection does not match the quote")]
    PcrSelectionMismatch,
    #[error("quote.pcrs digest count does not match selected PCRs")]
    PcrDigestCountMismatch,
    #[error("TPM PCR digest does not match the quote")]
    PcrDigestMismatch,
    #[error("quote.pcrs is truncated")]
    TruncatedPcrFile,
    #[error("quote.pcrs has trailing bytes")]
    TrailingPcrBytes,
}

/// Fail-closed errors raised while parsing HCLA and SEV-SNP report bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SnpParseError {
    #[error("SNP report length does not match")]
    ReportLength,
    #[error("HCLA blob is truncated")]
    HclaTooShort,
    #[error("HCLA signature does not match")]
    HclaMagicMismatch,
    #[error("HCLA request type does not match")]
    HclaRequestTypeMismatch,
    #[error("HCLA runtime JSON was not found")]
    RuntimeJsonNotFound,
    #[error("HCLA runtime JSON did not parse")]
    RuntimeJsonInvalid,
    #[error("HCLA runtime JSON is not an object")]
    RuntimeJsonNotObject,
}

/// Fail-closed errors raised while verifying AMD certificate and report evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SnpVerifyError {
    #[error("runtime JSON digest does not match report data")]
    RuntimeBindingDigestMismatch,
    #[error("runtime JSON report-data tail is nonzero")]
    RuntimeBindingTailNonZero,
    #[error("certificate did not parse")]
    CertificateParse,
    #[error("no PEM certificates were supplied")]
    NoBundleCertificates,
    #[error("VCEK selection failed")]
    VcekSelectionFailure,
    #[error("VCEK issuer did not select a pinned AMD root generation")]
    UnknownRootGeneration,
    #[error("AMD root material is invalid")]
    RootMaterialInvalid,
    #[error("certificate signature verification failed")]
    ChainSignatureInvalid,
    #[error("certificate is outside its validity window")]
    CertificateTimeInvalid,
    #[error("bundle CA does not match pinned root material")]
    BundleCaMismatch,
    #[error("AMD report signature reserved bytes are nonzero")]
    ReportSignatureReservedNonZero,
    #[error("AMD report signature scalar exceeds P-384 width")]
    ReportSignatureScalarOverflow,
    #[error("AMD report signature is invalid")]
    ReportSignatureInvalid,
    #[error("certificate algorithm is outside the supported profile")]
    UnsupportedCertificateAlgorithm,
    #[error("HCLA version is not allowed")]
    HclaVersionDisallowed,
    #[error("HCLA AK JWK is missing")]
    AkJwkMissing,
    #[error("HCLA AK JWK is invalid")]
    AkJwkInvalid,
    #[error("bundle AK public key is invalid")]
    AkKeyInvalid,
    #[error("bundle AK public key does not match HCLA AK JWK")]
    AkKeyMismatch,
    #[error("SNP report version is not allowed")]
    PolicyReportVersion,
    #[error("SNP report VMPL is not allowed")]
    PolicyVmpl,
    #[error("SNP report policy allows DEBUG")]
    PolicyDebugEnabled,
}

/// The CPU-evidence check at which a typed error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuAppraisalStage {
    Envelope,
    Hcla,
    Report,
    RuntimeBinding,
    AmdChain,
    SnpPolicy,
    AkBinding,
    Quote,
}

/// Fail-closed errors raised while appraising CPU-leg evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CpuLegError {
    #[error("CPU evidence failed at {stage:?}: {source}")]
    Tlv {
        stage: CpuAppraisalStage,
        #[source]
        source: TlvError,
    },
    #[error("CPU evidence failed at {stage:?}: {source}")]
    Binding {
        stage: CpuAppraisalStage,
        #[source]
        source: BindingError,
    },
    #[error("CPU evidence failed at {stage:?}: {source}")]
    SnpParse {
        stage: CpuAppraisalStage,
        #[source]
        source: SnpParseError,
    },
    #[error("CPU evidence failed at {stage:?}: {source}")]
    SnpVerify {
        stage: CpuAppraisalStage,
        #[source]
        source: SnpVerifyError,
    },
    #[error("CPU evidence failed at {stage:?}: {source}")]
    TpmQuote {
        stage: CpuAppraisalStage,
        #[source]
        source: TpmQuoteError,
    },
}

/// Stable GPU-appraisal classifications shared with the nvattest forwarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuAppraisalReason {
    NvattestUnavailable,
    NvattestIntegrityFailed,
    GpuNonceMismatch,
    GpuAppraisalFailed,
}

/// Fail-closed errors raised while appraising nvattest JSON claims.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GpuClaimsError {
    #[error("nvattest stdout is empty")]
    StdoutEmpty,
    #[error("nvattest stdout did not parse as JSON")]
    StdoutInvalidJson,
    #[error("nvattest stdout is not a JSON object")]
    StdoutNotObject,
    #[error("nvattest stdout is missing {key}")]
    MissingTopLevelKey { key: &'static str },
    #[error("nvattest return code is not green")]
    NonGreenReturncode,
    #[error("nvattest result code is not green")]
    NonGreenResultCode,
    #[error("nvattest result message is not green")]
    NonGreenResultMessage,
    #[error("nvattest claims do not have the required one-object shape")]
    ClaimsShape,
    #[error("nvattest detached EAT does not have the required shape")]
    DetachedEatShape,
    #[error("nvattest overall JWT does not have three segments")]
    JwtShape,
    #[error("nvattest overall JWT segment did not decode")]
    JwtSegmentDecode,
    #[error("nvattest overall JWT header is not an object")]
    JwtHeaderNotObject,
    #[error("nvattest overall JWT payload is not an object")]
    JwtPayloadNotObject,
    #[error("nvattest overall JWT header is missing alg")]
    JwtHeaderMissingAlgorithm,
    #[error("nvattest overall JWT payload is missing iss")]
    JwtPayloadMissingIssuer,
    #[error("nvattest overall JWT payload is missing overall result")]
    JwtPayloadMissingOverallResult,
    #[error("nvattest overall JWT algorithm is not accepted")]
    JwtAlgorithm,
    #[error("nvattest overall JWT issuer is not accepted")]
    JwtIssuer,
    #[error("nvattest overall JWT result is not literal true")]
    JwtOverallResult,
    #[error("nvattest claim is missing {key}")]
    ClaimMissingKey { key: &'static str },
    #[error("nvattest claim value does not match")]
    ClaimValueMismatch,
    #[error("nvattest claim identity value does not match")]
    ClaimIdentityMismatch,
    #[error("nvattest certificate-chain claim is not an object")]
    CertificateChainShape,
    #[error("nvattest certificate-chain field does not match")]
    CertificateChainField,
    #[error("nvattest claim string field is invalid")]
    ClaimStringField,
    #[error("SPP envelope metadata is not UTF-8")]
    EnvelopeFieldUtf8,
}
