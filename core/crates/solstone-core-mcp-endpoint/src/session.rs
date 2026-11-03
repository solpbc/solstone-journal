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
    sessions: Mutex<SessionState>,
}

struct SessionState {
    sessions: HashMap<String, SessionRecord>,
    next_admission_ordinal: u64,
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
    generation: Option<u64>,
    admission_ordinal: u64,
    listed_while_paused: bool,
}

/// A reason session creation or ownership validation was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionError {
    Randomness,
    #[allow(dead_code)]
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
            sessions: Mutex::new(SessionState {
                sessions: HashMap::new(),
                next_admission_ordinal: 0,
            }),
        }
    }

    /// Create one session bound to a durable opaque bearer-token identifier.
    pub(crate) fn create(&self, token_id: &str) -> Result<String, SessionError> {
        self.create_with_generation(token_id, None)
    }

    pub(crate) fn create_with_generation(
        &self,
        token_id: &str,
        generation: Option<u64>,
    ) -> Result<String, SessionError> {
        self.create_with_random_and_generation(token_id, generation, &SystemRandomSource)
    }

    #[allow(dead_code)]
    pub(crate) fn create_with_random(
        &self,
        token_id: &str,
        random: &dyn RandomSource,
    ) -> Result<String, SessionError> {
        self.create_with_random_and_generation(token_id, None, random)
    }

    pub(crate) fn create_with_random_and_generation(
        &self,
        token_id: &str,
        generation: Option<u64>,
        random: &dyn RandomSource,
    ) -> Result<String, SessionError> {
        let session_id = complete_session_id(random)?;
        let now = Instant::now();
        let mut state = self
            .sessions
            .lock()
            .map_err(|_| SessionError::Unavailable)?;
        purge_expired(&mut state.sessions, now);
        if state.sessions.contains_key(&session_id) {
            return Err(SessionError::Randomness);
        }
        if state.sessions.len() >= MAX_SESSIONS
            || state
                .sessions
                .values()
                .filter(|session| session.token_id == token_id)
                .count()
                >= MAX_SESSIONS_PER_PRINCIPAL
        {
            // 🔑 A bearer reclaims its own least-recently-used retained session;
            // equal last_used breaks strictly by the admission ordinal assigned under
            // this lock. Another bearer's row is never removed.
            let victim_id = state
                .sessions
                .iter()
                .filter(|(_, session)| session.token_id == token_id)
                .min_by(|(_, a), (_, b)| {
                    (a.last_used, a.admission_ordinal).cmp(&(b.last_used, b.admission_ordinal))
                })
                .map(|(id, _)| id.clone());
            let Some(victim_id) = victim_id else {
                return Err(SessionError::GlobalLimit);
            };
            state.sessions.remove(&victim_id);
        }
        let ordinal = state.next_admission_ordinal;
        state.next_admission_ordinal += 1;
        state.sessions.insert(
            session_id.clone(),
            SessionRecord {
                token_id: token_id.to_owned(),
                created_at: now,
                last_used: now,
                advertised_generation: None,
                generation,
                admission_ordinal: ordinal,
                listed_while_paused: false,
            },
        );
        Ok(session_id)
    }

    /// Verify that a supplied session remains live and belongs to this request's bearer token.
    pub(crate) fn validate(&self, session_id: &str, token_id: &str) -> Result<(), SessionError> {
        self.validate_with_generation(session_id, token_id, None)
    }

    pub(crate) fn validate_with_generation(
        &self,
        session_id: &str,
        token_id: &str,
        expected_generation: Option<u64>,
    ) -> Result<(), SessionError> {
        let now = Instant::now();
        let mut state = self
            .sessions
            .lock()
            .map_err(|_| SessionError::Unavailable)?;
        purge_expired(&mut state.sessions, now);
        let session = state
            .sessions
            .get_mut(session_id)
            .ok_or(SessionError::NotFound)?;
        if session.token_id != token_id {
            return Err(SessionError::Foreign);
        }
        if expected_generation.is_some() && session.generation != expected_generation {
            return Err(SessionError::NotFound);
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
        if let Ok(mut state) = self.sessions.lock()
            && let Some(session) = state.sessions.get_mut(session_id)
        {
            session.advertised_generation = Some(generation);
        }
    }

    pub(crate) fn record_listed_while_paused(&self, session_id: &str) {
        if let Ok(mut state) = self.sessions.lock()
            && let Some(session) = state.sessions.get_mut(session_id)
        {
            session.listed_while_paused = true;
        }
    }

    pub(crate) fn take_listed_while_paused(&self, session_id: &str) -> bool {
        if let Ok(mut state) = self.sessions.lock()
            && let Some(session) = state.sessions.get_mut(session_id)
        {
            let was = session.listed_while_paused;
            session.listed_while_paused = false;
            was
        } else {
            false
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
        let advertised = self.sessions.lock().ok().and_then(|state| {
            state
                .sessions
                .get(session_id)
                .map(|session| session.advertised_generation)
        });
        matches!(advertised, Some(Some(advertised)) if advertised != live_generation)
    }

    /// Delete one session only after its bearer-token ownership is verified.
    pub(crate) fn delete(&self, session_id: &str, token_id: &str) -> Result<(), SessionError> {
        let now = Instant::now();
        let mut state = self
            .sessions
            .lock()
            .map_err(|_| SessionError::Unavailable)?;
        purge_expired(&mut state.sessions, now);
        let session = state
            .sessions
            .get(session_id)
            .ok_or(SessionError::NotFound)?;
        if session.token_id != token_id {
            return Err(SessionError::Foreign);
        }
        state.sessions.remove(session_id);
        Ok(())
    }

    /// Clear all active sessions (e.g. on cutover).
    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.sessions.lock() {
            state.sessions.clear();
        }
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    fn len(&self) -> usize {
        self.sessions
            .lock()
            .expect("test session lock")
            .sessions
            .len()
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    fn created_at(&self, session_id: &str) -> Option<Instant> {
        self.sessions
            .lock()
            .expect("test session lock")
            .sessions
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
    async fn caps_reclaim_same_bearer_and_idle_expiry_recovers_capacity() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let principal_sessions = (0..MAX_SESSIONS_PER_PRINCIPAL)
            .map(|_| table.create_with_random("principal", &random).unwrap())
            .collect::<Vec<_>>();
        let seventeenth = table.create_with_random("principal", &random);
        assert!(seventeenth.is_ok());
        assert_eq!(table.len(), MAX_SESSIONS_PER_PRINCIPAL);
        assert_eq!(
            table.validate(&principal_sessions[0], "principal"),
            Err(SessionError::NotFound)
        );
        for id in &principal_sessions[1..] {
            assert!(table.validate(id, "principal").is_ok());
        }

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

    #[tokio::test(start_paused = true)]
    async fn sequential_admissions_for_one_bearer_reuse_slots_and_retain_newest_sixteen() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let mut ids = Vec::new();
        for _ in 0..33 {
            let id = table
                .create_with_random("bearer", &random)
                .expect("admission succeeds");
            assert!(table.validate(&id, "bearer").is_ok());
            assert!(table.len() <= MAX_SESSIONS_PER_PRINCIPAL);
            ids.push(id);
            let retain_count = ids.len().min(MAX_SESSIONS_PER_PRINCIPAL);
            let cutoff = ids.len() - retain_count;
            for older_id in &ids[..cutoff] {
                assert_eq!(
                    table.validate(older_id, "bearer"),
                    Err(SessionError::NotFound)
                );
            }
            assert!(table.validate(ids.last().unwrap(), "bearer").is_ok());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn lru_eviction_follows_successful_validation_refresh() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let ids = (0..MAX_SESSIONS_PER_PRINCIPAL)
            .map(|_| table.create_with_random("bearer", &random).unwrap())
            .collect::<Vec<_>>();
        advance(Duration::from_secs(60)).await;
        assert!(table.validate(&ids[0], "bearer").is_ok());
        let seventeenth = table.create_with_random("bearer", &random).unwrap();
        assert!(table.validate(&ids[0], "bearer").is_ok());
        assert_eq!(
            table.validate(&ids[1], "bearer"),
            Err(SessionError::NotFound)
        );
        assert!(table.validate(&seventeenth, "bearer").is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn equal_last_used_evicts_in_strict_admission_ordinal_order() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let ids = (0..MAX_SESSIONS_PER_PRINCIPAL)
            .map(|_| table.create_with_random("bearer", &random).unwrap())
            .collect::<Vec<_>>();
        for id in ids.iter().rev() {
            assert!(table.validate(id, "bearer").is_ok());
        }
        let seventeenth = table.create_with_random("bearer", &random).unwrap();
        assert_eq!(
            table.validate(&ids[0], "bearer"),
            Err(SessionError::NotFound)
        );
        assert!(table.validate(&ids[1], "bearer").is_ok());
        assert!(table.validate(&seventeenth, "bearer").is_ok());

        let eighteenth = table.create_with_random("bearer", &random).unwrap();
        assert_eq!(
            table.validate(&ids[1], "bearer"),
            Err(SessionError::NotFound)
        );
        assert!(table.validate(&ids[2], "bearer").is_ok());
        assert!(table.validate(&eighteenth, "bearer").is_ok());
    }

    #[test]
    fn concurrent_same_bearer_admissions_maintain_capacity_and_unique_ids() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;

        let table = Arc::new(SessionTable::new());
        let random = Arc::new(SequentialRandom::new());
        let barrier = Arc::new(Barrier::new(32));
        let results = Arc::new(Mutex::new(Vec::new()));

        thread::scope(|scope| {
            for _ in 0..32 {
                let table = Arc::clone(&table);
                let random = Arc::clone(&random);
                let barrier = Arc::clone(&barrier);
                let results = Arc::clone(&results);
                scope.spawn(move || {
                    barrier.wait();
                    let id = table
                        .create_with_random("concurrent-bearer", &*random)
                        .expect("admission succeeds");
                    results.lock().unwrap().push(id);
                });
            }
        });

        let ids = results.lock().unwrap().clone();
        assert_eq!(ids.len(), 32);
        let unique = ids.into_iter().collect::<HashSet<_>>();
        assert_eq!(unique.len(), 32);
        assert!(table.len() <= MAX_SESSIONS_PER_PRINCIPAL);
    }

    #[tokio::test(start_paused = true)]
    async fn unsuccessful_and_mismatched_generation_validations_do_not_refresh_lru() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let first_id = table
            .create_with_random_and_generation("bearer", Some(10), &random)
            .unwrap();
        let mut ids = vec![first_id.clone()];
        for _ in 1..MAX_SESSIONS_PER_PRINCIPAL {
            ids.push(table.create_with_random("bearer", &random).unwrap());
        }
        advance(Duration::from_secs(60)).await;
        assert_eq!(
            table.validate(&first_id, "different-bearer"),
            Err(SessionError::Foreign)
        );
        assert_eq!(
            table.validate_with_generation(&first_id, "bearer", Some(99)),
            Err(SessionError::NotFound)
        );
        let seventeenth = table.create_with_random("bearer", &random).unwrap();
        assert_eq!(
            table.validate(&first_id, "bearer"),
            Err(SessionError::NotFound)
        );
        assert!(table.validate(&ids[1], "bearer").is_ok());
        assert!(table.validate(&seventeenth, "bearer").is_ok());
    }

    #[tokio::test]
    async fn session_reclamation_is_scoped_strictly_to_the_requesting_bearer() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let b_ids = (0..5)
            .map(|_| table.create_with_random("bearer-b", &random).unwrap())
            .collect::<Vec<_>>();
        let a_ids = (0..MAX_SESSIONS_PER_PRINCIPAL)
            .map(|_| table.create_with_random("bearer-a", &random).unwrap())
            .collect::<Vec<_>>();
        let a_extra = table.create_with_random("bearer-a", &random).unwrap();

        for b_id in &b_ids {
            assert!(table.validate(b_id, "bearer-b").is_ok());
        }
        assert_eq!(
            table.validate(&b_ids[0], "bearer-a"),
            Err(SessionError::Foreign)
        );
        assert_eq!(
            table.delete(&b_ids[0], "bearer-a"),
            Err(SessionError::Foreign)
        );
        assert!(table.validate(&b_ids[0], "bearer-b").is_ok());
        assert!(table.validate(&a_ids[0], "bearer-a").is_err());
        assert!(table.validate(&a_extra, "bearer-a").is_ok());
    }

    #[tokio::test]
    async fn global_limit_reclaims_held_bearer_session_or_rejects_bearer_with_none() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let held_ids = (0..3)
            .map(|_| table.create_with_random("held-bearer", &random).unwrap())
            .collect::<Vec<_>>();
        let other_ids = (0..(MAX_SESSIONS - 3))
            .map(|index| {
                table
                    .create_with_random(&format!("other-{index}"), &random)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(table.len(), MAX_SESSIONS);

        let reclaimed = table.create_with_random("held-bearer", &random).unwrap();
        assert_eq!(table.len(), MAX_SESSIONS);
        assert_eq!(
            table.validate(&held_ids[0], "held-bearer"),
            Err(SessionError::NotFound)
        );
        assert!(table.validate(&held_ids[1], "held-bearer").is_ok());
        assert!(table.validate(&held_ids[2], "held-bearer").is_ok());
        assert!(table.validate(&reclaimed, "held-bearer").is_ok());

        for (index, id) in other_ids.iter().enumerate() {
            assert!(table.validate(id, &format!("other-{index}")).is_ok());
        }

        assert_eq!(
            table.create_with_random("empty-bearer", &random),
            Err(SessionError::GlobalLimit)
        );
        assert_eq!(table.len(), MAX_SESSIONS);

        assert!(table.validate(&held_ids[1], "held-bearer").is_ok());
        assert!(table.validate(&held_ids[2], "held-bearer").is_ok());
        assert!(table.validate(&reclaimed, "held-bearer").is_ok());
        for (index, id) in other_ids.iter().enumerate() {
            assert!(table.validate(id, &format!("other-{index}")).is_ok());
        }
    }

    struct ReplayRandom(Vec<u8>);

    impl RandomSource for ReplayRandom {
        fn fill(&self, bytes: &mut [u8]) -> Result<usize, RandomSourceError> {
            let len = self.0.len().min(bytes.len());
            bytes[..len].copy_from_slice(&self.0[..len]);
            Ok(len)
        }
    }

    #[tokio::test]
    async fn randomness_failure_or_victim_id_collision_aborts_without_eviction() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let ids = (0..MAX_SESSIONS_PER_PRINCIPAL)
            .map(|_| table.create_with_random("bearer", &random).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            table.create_with_random("bearer", &ShortRandom),
            Err(SessionError::Randomness)
        );
        for id in &ids {
            assert!(table.validate(id, "bearer").is_ok());
        }
        assert_eq!(table.len(), MAX_SESSIONS_PER_PRINCIPAL);

        let victim_bytes = URL_SAFE_NO_PAD.decode(&ids[0]).unwrap();
        let replay = ReplayRandom(victim_bytes);
        assert_eq!(
            table.create_with_random("bearer", &replay),
            Err(SessionError::Randomness)
        );
        for id in &ids {
            assert!(table.validate(id, "bearer").is_ok());
        }
        assert_eq!(table.len(), MAX_SESSIONS_PER_PRINCIPAL);
    }

    #[tokio::test(start_paused = true)]
    async fn reclaimed_session_resets_advertised_generation_and_adopts_new_generation() {
        let table = SessionTable::new();
        let random = SequentialRandom::new();
        let first_id = table
            .create_with_random_and_generation("bearer", Some(100), &random)
            .unwrap();
        table.record_advertised_generation(&first_id, Some(100));

        let mut ids = vec![first_id.clone()];
        for _ in 1..MAX_SESSIONS_PER_PRINCIPAL {
            ids.push(table.create_with_random("bearer", &random).unwrap());
        }

        let new_id = table
            .create_with_random_and_generation("bearer", Some(200), &random)
            .unwrap();

        assert_eq!(
            table.validate(&first_id, "bearer"),
            Err(SessionError::NotFound)
        );

        assert!(!table.tools_list_is_stale(&new_id, Some(200)));
        assert!(!table.tools_list_is_stale(&new_id, Some(100)));

        assert_eq!(
            table.validate_with_generation(&new_id, "bearer", Some(999)),
            Err(SessionError::NotFound)
        );
        assert!(
            table
                .validate_with_generation(&new_id, "bearer", Some(200))
                .is_ok()
        );

        table.record_advertised_generation(&new_id, Some(200));
        assert!(!table.tools_list_is_stale(&new_id, Some(200)));
        assert!(table.tools_list_is_stale(&new_id, Some(300)));
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
