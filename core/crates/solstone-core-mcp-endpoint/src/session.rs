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
    /// The grant generation whose tool list this session was last served.
    ///
    /// 🔑 This is the state half of `notifications/tools/list_changed`, and it
    /// is the half that has to exist before the notification can: a client
    /// caches schemas, so the server needs to know which grant the cached
    /// schemas describe. ⚠ At this revision there is no server→client channel
    /// to deliver a notification over — `GET /mcp` is 405, there is no SSE
    /// stream — so the one consumer is the in-band staleness signal on the
    /// next call. ⛔ `listChanged` is therefore not advertised in `initialize`:
    /// advertising a notification we cannot send would be worse than silence.
    ///
    /// ⚠ **Two nested options, and collapsing them is a real miss.** The outer
    /// `None` means this session has never been served a tool list, so it has
    /// nothing cached to be wrong about. `Some(None)` means it *was* served one
    /// while it had no enforceable permission — an empty list — and a grant
    /// appearing afterwards leaves it holding exactly the stale schema directive
    /// 3 exists to remove.
    advertised_generation: Option<Option<u64>>,
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
                advertised_generation: None,
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

    /// Remember which grant generation this session's tool list described.
    ///
    /// Best effort: a session that has expired between the list and this call
    /// simply has nothing to remember.
    pub(crate) fn record_advertised_generation(&self, session_id: &str, generation: Option<u64>) {
        if let Ok(mut sessions) = self.sessions.lock()
            && let Some(session) = sessions.get_mut(session_id)
        {
            session.advertised_generation = Some(generation);
        }
    }

    /// Whether this session holds a tool list that no longer describes its grant.
    ///
    /// ⚠ A session that has never listed tools is not stale — it has nothing
    /// cached to be wrong about — so this answers false rather than true.
    /// ✅ Telling a connection that its own grant moved discloses nothing: it
    /// could establish the same fact by calling `tools/list` again, and the
    /// change was the owner's own deliberate act.
    pub(crate) fn tools_list_is_stale(
        &self,
        session_id: &str,
        live_generation: Option<u64>,
    ) -> bool {
        let advertised = self.sessions.lock().ok().and_then(|sessions| {
            sessions
                .get(session_id)
                .map(|session| session.advertised_generation)
        });
        matches!(advertised, Some(Some(advertised)) if advertised != live_generation)
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

    #[tokio::test]
    async fn a_session_that_never_listed_tools_is_not_stale_but_one_served_an_empty_list_is() {
        let sessions = SessionTable::new();
        let session = sessions.create("token-1").expect("session is created");

        // Never listed: nothing cached, so nothing to be wrong about.
        assert!(!sessions.tools_list_is_stale(&session, Some(7)));
        assert!(!sessions.tools_list_is_stale(&session, None));

        // Listed under a real grant.
        sessions.record_advertised_generation(&session, Some(7));
        assert!(!sessions.tools_list_is_stale(&session, Some(7)));
        assert!(sessions.tools_list_is_stale(&session, Some(8)));
        assert!(sessions.tools_list_is_stale(&session, None));

        // \u26a0 Listed while it had no permission at all \u2014 an EMPTY list. A grant
        // appearing afterwards leaves it holding exactly the stale schema
        // directive 3 exists to remove, and a single `Option` would miss it.
        sessions.record_advertised_generation(&session, None);
        assert!(!sessions.tools_list_is_stale(&session, None));
        assert!(sessions.tools_list_is_stale(&session, Some(1)));

        // An unknown session is never stale.
        assert!(!sessions.tools_list_is_stale("no-such-session", Some(1)));
    }

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
