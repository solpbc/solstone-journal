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

#[cfg(test)]
mod tests {
    use super::run_blocking;

    #[tokio::test]
    async fn a_store_operation_answers_and_a_panic_comes_back_as_an_error() {
        assert_eq!(run_blocking(|| 2 + 2).await.ok(), Some(4));
        // A handler can answer for a panicking store operation instead of
        // losing the request.
        let error = run_blocking(|| -> u8 { panic!("store failed") })
            .await
            .expect_err("a panic is an error");
        assert!(error.is_panic());
    }
}
