// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ephemeral, bearer-token-bound MCP session admission.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio::time::Instant;

use crate::tokens::{RandomSource, SystemRandomSource};

const SESSION_ID_BYTES: usize = 16;
const MAX_SESSIONS_PER_PRINCIPAL: usize = 16;
const MAX_SESSIONS: usize = 1024;
const IDLE_EXPIRY: Duration = Duration::from_secs(30 * 60);

/// Shared, process-local MCP session state.
pub(crate) struct SessionTable {
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

struct SessionRecord {
    token_id: String,
    created_at: Instant,
    last_used: Instant,
}

/// A reason session creation or ownership validation was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionError {
    Randomness,
    PrincipalLimit,
    GlobalLimit,
    NotFound,
    Foreign,
    Unavailable,
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Randomness => "could not obtain complete MCP session randomness",
            Self::PrincipalLimit => "MCP session limit for this bearer token is reached",
            Self::GlobalLimit => "MCP global session limit is reached",
            Self::NotFound => "MCP session is invalid or expired",
            Self::Foreign => "MCP session belongs to a different bearer token",
            Self::Unavailable => "MCP session state is unavailable",
        };
        formatter.write_str(message)
    }
}

impl SessionTable {
    pub(crate) fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Create one session bound to a durable opaque bearer-token identifier.
    pub(crate) fn create(&self, token_id: &str) -> Result<String, SessionError> {
        self.create_with_random(token_id, &SystemRandomSource)
    }

    pub(crate) fn create_with_random(
        &self,
        token_id: &str,
        random: &dyn RandomSource,
    ) -> Result<String, SessionError> {
        let session_id = complete_session_id(random)?;
        let now = Instant::now();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::Unavailable)?;
        purge_expired(&mut sessions, now);
        if sessions.len() >= MAX_SESSIONS {
            return Err(SessionError::GlobalLimit);
        }
        if sessions
            .values()
            .filter(|session| session.token_id == token_id)
            .count()
            >= MAX_SESSIONS_PER_PRINCIPAL
        {
            return Err(SessionError::PrincipalLimit);
        }
        if sessions.contains_key(&session_id) {
            return Err(SessionError::Randomness);
        }
        sessions.insert(
            session_id.clone(),
            SessionRecord {
                token_id: token_id.to_owned(),
                created_at: now,
                last_used: now,
            },
        );
        Ok(session_id)
    }

    /// Verify that a supplied session remains live and belongs to this request's bearer token.
    pub(crate) fn validate(&self, session_id: &str, token_id: &str) -> Result<(), SessionError> {
        let now = Instant::now();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::Unavailable)?;
        purge_expired(&mut sessions, now);
        let session = sessions.get_mut(session_id).ok_or(SessionError::NotFound)?;
        if session.token_id != token_id {
            return Err(SessionError::Foreign);
        }
        debug_assert!(session.created_at <= now);
        session.last_used = now;
        Ok(())
    }

    /// Delete one session only after its bearer-token ownership is verified.
    pub(crate) fn delete(&self, session_id: &str, token_id: &str) -> Result<(), SessionError> {
        let now = Instant::now();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::Unavailable)?;
        purge_expired(&mut sessions, now);
        let session = sessions.get(session_id).ok_or(SessionError::NotFound)?;
        if session.token_id != token_id {
            return Err(SessionError::Foreign);
        }
        sessions.remove(session_id);
        Ok(())
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    fn len(&self) -> usize {
        self.sessions.lock().expect("test session lock").len()
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    fn created_at(&self, session_id: &str) -> Option<Instant> {
        self.sessions
            .lock()
            .expect("test session lock")
            .get(session_id)
            .map(|session| session.created_at)
    }
}

fn complete_session_id(random: &dyn RandomSource) -> Result<String, SessionError> {
    let mut bytes = [0_u8; SESSION_ID_BYTES];
    let written = random
        .fill(&mut bytes)
        .map_err(|_| SessionError::Randomness)?;
    if written != bytes.len() {
        return Err(SessionError::Randomness);
    }
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn purge_expired(sessions: &mut HashMap<String, SessionRecord>, now: Instant) {
    sessions.retain(|_, session| now.saturating_duration_since(session.last_used) < IDLE_EXPIRY);
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use tokio::time::{Duration, advance};

    use crate::tokens::{RandomSource, RandomSourceError};

    use super::{MAX_SESSIONS, MAX_SESSIONS_PER_PRINCIPAL, SessionError, SessionTable};

    struct SequentialRandom(AtomicUsize);

    impl SequentialRandom {
        fn new() -> Self {
            Self(AtomicUsize::new(0))
        }
    }

    impl RandomSource for SequentialRandom {
        fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError> {
            bytes.fill(0);
            let sequence = self.0.fetch_add(1, Ordering::Relaxed).to_be_bytes();
            let offset = bytes.len() - sequence.len();
            bytes[offset..].copy_from_slice(&sequence);
            Ok(bytes.len())
        }
    }

    struct ShortRandom;

    impl RandomSource for ShortRandom {
        fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError> {
            bytes[..15].fill(0x5a);
            Ok(15)
        }
    }

    #[tokio::test]
    async fn session_ids_have_at_least_128_random_bits_and_reject_short_randomness() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let id = table.create_with_random("token-a", &random).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(&id).unwrap().len(), 16);
        assert!(table.created_at(&id).is_some());
        assert_eq!(
            table.create_with_random("token-a", &ShortRandom),
            Err(SessionError::Randomness)
        );
        assert_eq!(table.len(), 1);
    }

    #[tokio::test]
    async fn sessions_are_bound_to_the_token_identifier_that_created_them() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let id = table.create_with_random("token-a", &random).unwrap();
        assert_eq!(table.validate(&id, "token-b"), Err(SessionError::Foreign));
        assert!(table.validate(&id, "token-a").is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn caps_reject_without_eviction_and_idle_expiry_recovers_capacity() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let principal_sessions = (0..MAX_SESSIONS_PER_PRINCIPAL)
            .map(|_| table.create_with_random("principal", &random).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            table.create_with_random("principal", &random),
            Err(SessionError::PrincipalLimit)
        );
        assert!(
            principal_sessions
                .iter()
                .all(|id| table.validate(id, "principal").is_ok())
        );

        let global = SessionTable::new();
        for index in 0..MAX_SESSIONS {
            global
                .create_with_random(&format!("token-{index}"), &random)
                .unwrap();
        }
        assert_eq!(
            global.create_with_random("overflow", &random),
            Err(SessionError::GlobalLimit)
        );
        assert_eq!(global.len(), MAX_SESSIONS);

        advance(Duration::from_secs(30 * 60)).await;
        assert!(global.create_with_random("recovered", &random).is_ok());
        assert_eq!(global.len(), 1);
    }

    #[tokio::test]
    async fn delete_only_removes_a_session_owned_by_the_current_bearer_token() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let id = table.create_with_random("token-a", &random).unwrap();
        assert_eq!(table.delete(&id, "token-b"), Err(SessionError::Foreign));
        assert!(table.validate(&id, "token-a").is_ok());
        assert!(table.delete(&id, "token-a").is_ok());
        assert_eq!(table.validate(&id, "token-a"), Err(SessionError::NotFound));
    }
}
