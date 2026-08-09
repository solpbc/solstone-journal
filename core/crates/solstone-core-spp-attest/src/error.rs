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
