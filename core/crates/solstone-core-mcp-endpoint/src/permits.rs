// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded admission for accepted MCP listener connections.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The maximum number of accepted MCP connections in every pre- and post-TLS stage.
pub(crate) const CONNECTION_PERMITS: usize = 256;

/// Construct the listener's global connection-admission pool.
pub(crate) fn connection_permit_pool() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(CONNECTION_PERMITS))
}

/// Acquire one connection permit without waiting or queuing.
pub(crate) fn try_acquire_connection_permit(pool: &Arc<Semaphore>) -> Option<OwnedSemaphorePermit> {
    Arc::clone(pool).try_acquire_owned().ok()
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{CONNECTION_PERMITS, connection_permit_pool, try_acquire_connection_permit};

    #[tokio::test(start_paused = true)]
    async fn exhausted_pool_rejects_the_next_connection_without_waiting() {
        let pool = connection_permit_pool();
        let permits = (0..CONNECTION_PERMITS)
            .map(|_| try_acquire_connection_permit(&pool).expect("capacity remains"))
            .collect::<Vec<_>>();

        assert_eq!(pool.available_permits(), 0);
        assert!(try_acquire_connection_permit(&pool).is_none());
        assert_eq!(pool.available_permits(), 0);
        drop(permits);
    }

    #[tokio::test(start_paused = true)]
    async fn released_capacity_is_recovered_exactly() {
        let pool = connection_permit_pool();
        let mut permits = (0..CONNECTION_PERMITS)
            .map(|_| try_acquire_connection_permit(&pool).expect("capacity remains"))
            .collect::<Vec<_>>();

        let released = permits.split_off(CONNECTION_PERMITS - 10);
        drop(released);
        assert_eq!(pool.available_permits(), 10);
        let recovered = (0..10)
            .map(|_| try_acquire_connection_permit(&pool).expect("released capacity admits"))
            .collect::<Vec<_>>();
        assert!(try_acquire_connection_permit(&pool).is_none());
        drop(recovered);
        drop(permits);
    }
}
