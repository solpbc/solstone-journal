// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The embedded owner's maintained recurrence authority, without an importer dependency.

use axum::{body::Bytes, response::Response};
use std::{future::Future, pin::Pin, sync::Arc};

pub type SourceSyncResponse = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;

pub trait SourceSyncAuthority: Send + Sync {
    fn get(&self) -> SourceSyncResponse;
    fn update(&self, body: Bytes) -> SourceSyncResponse;
}

#[derive(Clone)]
pub struct SourceSyncDelegate(Arc<dyn SourceSyncAuthority>);

impl SourceSyncDelegate {
    pub fn new(authority: Arc<dyn SourceSyncAuthority>) -> Self {
        Self(authority)
    }
    pub async fn get(&self) -> Response {
        self.0.get().await
    }
    pub async fn update(&self, body: Bytes) -> Response {
        self.0.update(body).await
    }
}
