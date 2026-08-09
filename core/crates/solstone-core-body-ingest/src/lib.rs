// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-only Apple Health and Oura ingress into immutable native body bundles.

mod apple;
mod approval;
mod bounded_file;
mod bundle;
mod oura;
mod oura_connect;
mod oura_sync;

pub use apple::{AppleImportOptions, detect_apple_source, preview_apple, save_apple};
pub use bundle::{BodyIngestError, BodyIngestErrorKind, BodyIngestReport};
pub use oura::{
    OURA_SYNC_ENDPOINTS, OuraDocuments, OuraImportOptions, OuraNormalizedRow,
    normalize_oura_documents, parse_oura_source, preview_oura_source, save_oura_source,
};
pub use oura_connect::{OuraConnectOptions, OuraConnectReport, connect_oura};
pub use oura_sync::{
    OuraEndpointIssue, OuraEndpointIssueKind, OuraSyncOptions, OuraSyncReport, sync_oura,
};
