// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RatlsContractError {
    #[error("truncated DER length")]
    TruncatedLength,
    #[error("invalid DER length")]
    InvalidLength,
    #[error("non-minimal DER length")]
    NonMinimalLength,
    #[error("expected DER tag")]
    UnexpectedTag,
    #[error("truncated DER value")]
    TruncatedValue,
    #[error("trailing bytes after DER sequence")]
    TrailingBytes,
    #[error("non-minimal DER integer")]
    NonMinimalInteger,
    #[error("unexpected field in DER sequence")]
    UnexpectedField,
    #[error("unsupported evidence version")]
    UnsupportedVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("confidential attestation rejected ({reason_code})")]
pub struct RatlsVerificationError {
    pub reason_code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("confidential attestation rejected ({reason_code})")]
pub struct RatlsChannelError {
    pub reason_code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("composite appraisal failed ({reason_code})")]
pub struct CompositeVerificationError {
    pub reason_code: &'static str,
}
