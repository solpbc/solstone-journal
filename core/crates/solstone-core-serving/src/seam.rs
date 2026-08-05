// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The narrow async-to-sync boundary for trust-lock-backed store operations.

/// Run one synchronous store operation on Tokio's blocking pool.
///
/// Trust-lock guards are `!Send` (they contain `PhantomData<*const ()>`), so a
/// handler that holds one across an `.await` already fails to compile: Axum
/// handler futures must be `Send`. This seam keeps each acquire-to-drop
/// lifecycle inside one synchronous closure instead.
///
/// `solstone_core_convey_http::gate::require_access` currently returns `true`
/// for both `AccessBasis` variants, and the substrate does not require a
/// handler to extract or consult `Extension<AccessBasis>` before serving. That
/// authorization finding is recorded here only; this crate does not change it.
pub async fn run_blocking<F, T>(f: F) -> Result<T, tokio::task::JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await
}
