// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private-link service-token renewal.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::whole_journal_restore::{
    begin_target_use_bound, whole_journal_restore_blocks, whole_journal_restore_generation,
};
use solstone_core_sol_link::ca::jid_from_spki;
use solstone_core_sol_link::service_identity::{ServiceIdentityError, load_service_identity_ca};

use crate::health::{ServiceTokenHealth, service_token_health_is_refused};
use crate::link_state_files::{
    LinkServiceTokenRead, LinkStateRead, load_link_service_token, load_link_state,
};
use crate::private_link::{EnrollError, EnrollMode, enroll_home, enrollment_ca, relay_url};
use crate::relay_access::{decode_base64url, mutate_service_configuration};
use crate::service_token_write::save_service_token_at_root_unlocked;

pub const SERVICE_TOKEN_RENEWAL_TICK: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipClass {
    RootUnavailable,
    Posture,
    TokenMissing,
    TokenUnreadable,
    TokenMalformed,
    IdentityAbsent,
    IdentityUnreadable,
    IdentityMalformed,
    CaAbsent,
    CaUnloadable,
    InstanceMismatch,
}

impl SkipClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RootUnavailable => "root_unavailable",
            Self::Posture => "posture",
            Self::TokenMissing => "token_missing",
            Self::TokenUnreadable => "token_unreadable",
            Self::TokenMalformed => "token_malformed",
            Self::IdentityAbsent => "identity_absent",
            Self::IdentityUnreadable => "identity_unreadable",
            Self::IdentityMalformed => "identity_malformed",
            Self::CaAbsent => "ca_absent",
            Self::CaUnloadable => "ca_unloadable",
            Self::InstanceMismatch => "instance_mismatch",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Trigger {
    Age,
    Expired,
    Undecodable,
    UnknownKey,
    /// The token carries no `pop` claim: the relay has no proof of
    /// possession on record for this home yet.
    Unproven,
    /// The relay refused a proven token whose key it still publishes, so a
    /// newer token exists that this home does not hold.
    Refused,
}

impl Trigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Age => "age",
            Self::Expired => "expired",
            Self::Undecodable => "undecodable",
            Self::UnknownKey => "unknown_key",
            Self::Unproven => "unproven",
            Self::Refused => "refused",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Renewed,
    PublishedUnconfirmed,
    Unreachable,
    Rejected { status: u16 },
    NotEnrolled,
    InvalidResponse,
    WriteFailed,
    ProofUnavailable,
    Superseded,
}

impl Outcome {
    pub fn as_str(&self) -> String {
        match self {
            Self::Renewed => "renewed".to_owned(),
            Self::PublishedUnconfirmed => "published_unconfirmed".to_owned(),
            Self::Unreachable => "unreachable".to_owned(),
            Self::Rejected { status } => format!("rejected {status}"),
            Self::NotEnrolled => "not_enrolled".to_owned(),
            Self::InvalidResponse => "invalid_response".to_owned(),
            Self::WriteFailed => "write_failed".to_owned(),
            Self::ProofUnavailable => "proof_unavailable".to_owned(),
            Self::Superseded => "superseded".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenewalStep {
    Current,
    Skipped { class: SkipClass },
    Blocked,
    Attempted { trigger: Trigger, outcome: Outcome },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JwksSet {
    Healthy { kids: Vec<String> },
    Unhealthy,
}

#[derive(Clone)]
pub struct RenewalClock {
    pub now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub now_mono: Arc<dyn Fn() -> Duration + Send + Sync>,
}

impl RenewalClock {
    pub fn production() -> Self {
        let start = std::time::Instant::now();
        Self {
            now_unix: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            }),
            now_mono: Arc::new(move || start.elapsed()),
        }
    }
}

pub type EnrollFn =
    Arc<dyn Fn(&str, &str, &str, EnrollMode) -> Result<String, EnrollError> + Send + Sync>;

#[derive(Clone)]
pub struct RenewalConfig {
    pub journal_root: PathBuf,
    pub enroll: EnrollFn,
    pub jwks_fetch: Arc<dyn Fn(&str) -> JwksSet + Send + Sync>,
    pub health_read: Arc<dyn Fn() -> ServiceTokenHealth + Send + Sync>,
    pub clock: RenewalClock,
    pub tick: Duration,
    pub jitter: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    pub before_commit: Arc<dyn Fn() + Send + Sync>,
}

impl RenewalConfig {
    pub fn production(
        journal_root: PathBuf,
        health_read: Arc<dyn Fn() -> ServiceTokenHealth + Send + Sync>,
    ) -> Self {
        let ca_root = journal_root.clone();
        Self {
            journal_root,
            enroll: Arc::new(move |url, id, _pem, mode| {
                enroll_home(url, id, &enrollment_ca(&ca_root)?, mode)
            }),
            jwks_fetch: Arc::new(fetch_jwks_production),
            health_read,
            clock: RenewalClock::production(),
            tick: SERVICE_TOKEN_RENEWAL_TICK,
            jitter: Arc::new(|base| {
                let extra_ppm = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos()
                    % 250_001;
                let extra_nanos = (base.as_nanos() * extra_ppm as u128) / 1_000_000;
                base + Duration::from_nanos(extra_nanos as u64)
            }),
            before_commit: Arc::new(|| {}),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RenewalState {
    pub consecutive_ordinary_failures: u32,
    pub backoff_deadline: Duration,
    pub floor_end: Duration,
    pub last_log_class: Option<SkipClass>,
    pub last_log_outcome_str: Option<String>,
}

pub fn share_bp(instance_id: &str) -> u16 {
    let digest = Sha256::digest(instance_id.as_bytes());
    let mut hi64_bytes = [0u8; 8];
    hi64_bytes.copy_from_slice(&digest[..8]);
    let hi64 = u64::from_be_bytes(hi64_bytes);
    (7500 + (hi64 % 1001)) as u16
}

pub fn age_reached(iat: i64, exp: i64, share_bp: u16, now_unix: i64) -> bool {
    let Some(lifetime) = exp.checked_sub(iat) else {
        return false;
    };
    if lifetime <= 0 {
        return false;
    }
    let Some(product) = lifetime.checked_mul(share_bp as i64) else {
        return false;
    };
    let offset = product / 10_000;
    let Some(threshold) = iat.checked_add(offset) else {
        return false;
    };
    now_unix >= threshold
}

pub fn classify_jwks(status: u16, body: &[u8]) -> JwksSet {
    if status != 200 || body.len() > 64 * 1024 {
        return JwksSet::Unhealthy;
    }
    let Ok(val) = serde_json::from_slice::<Value>(body) else {
        return JwksSet::Unhealthy;
    };
    let Some(keys) = val.get("keys").and_then(Value::as_array) else {
        return JwksSet::Unhealthy;
    };
    if keys.is_empty() {
        return JwksSet::Unhealthy;
    }
    let mut kids = Vec::with_capacity(keys.len());
    for key in keys {
        let Some(kid) = key
            .as_object()
            .and_then(|o| o.get("kid"))
            .and_then(Value::as_str)
        else {
            return JwksSet::Unhealthy;
        };
        kids.push(kid.to_owned());
    }
    JwksSet::Healthy { kids }
}

fn fetch_jwks_production(relay_base_url: &str) -> JwksSet {
    use std::io::Read;
    let agent = crate::relay_access::build_isolated_ureq_agent(Duration::from_secs(30));
    let url = format!(
        "{}/.well-known/jwks.json",
        relay_base_url.trim_end_matches('/')
    );
    let response = match agent.get(&url).header("User-Agent", "").call() {
        Ok(res) => res,
        Err(_) => return JwksSet::Unhealthy,
    };
    let status = response.status().as_u16();
    let mut reader = response.into_body().into_reader();
    let mut body = Vec::new();
    let mut chunk = [0u8; 4096];
    while body.len() <= 64 * 1024 {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => return JwksSet::Unhealthy,
        }
    }
    if body.len() > 64 * 1024 {
        return JwksSet::Unhealthy;
    }
    classify_jwks(status, &body)
}

struct TokenMetadata {
    kid: Option<String>,
    instance_id: Option<String>,
    sub: Option<String>,
    scope: Option<String>,
    /// Minted on a proof of possession the relay verified.
    pop: bool,
}

fn parse_token_metadata(token: &str) -> TokenMetadata {
    let mut parts = token.split('.');
    let header_b64 = parts.next();
    let payload_b64 = parts.next();
    let _sig = parts.next();
    if parts.next().is_some() {
        return TokenMetadata {
            kid: None,
            instance_id: None,
            sub: None,
            scope: None,
            pop: false,
        };
    }
    let kid = header_b64
        .and_then(decode_base64url)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|val| val.get("kid").and_then(Value::as_str).map(str::to_owned));

    let (instance_id, sub, scope, pop) = payload_b64
        .and_then(decode_base64url)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map(|val| {
            let instance_id = val
                .get("instance_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let sub = val.get("sub").and_then(Value::as_str).map(str::to_owned);
            let scope = val.get("scope").and_then(Value::as_str).map(str::to_owned);
            let pop = val.get("pop").and_then(Value::as_bool) == Some(true);
            (instance_id, sub, scope, pop)
        })
        .unwrap_or((None, None, None, false));

    TokenMetadata {
        kid,
        instance_id,
        sub,
        scope,
        pop,
    }
}

fn validate_new_token(
    new_token: &str,
    expected_instance_id: &str,
    now_unix: i64,
    jwks_fetch: &Arc<dyn Fn(&str) -> JwksSet + Send + Sync>,
    relay_base: &str,
) -> bool {
    let Some(claims) = spl_core::jwt::decode_unverified_claims(new_token) else {
        return false;
    };
    if claims.iat >= claims.exp {
        return false;
    }
    let Some(lifetime) = claims.exp.checked_sub(claims.iat) else {
        return false;
    };
    if lifetime < 86_400 {
        return false;
    }
    let meta = parse_token_metadata(new_token);
    let Some(new_kid) = meta.kid else {
        return false;
    };
    if meta.instance_id.as_deref() != Some(expected_instance_id) {
        return false;
    }
    if meta.sub.as_deref() != Some(&format!("home:{expected_instance_id}")) {
        return false;
    }
    if meta.scope.as_deref() != Some("session.listen") {
        return false;
    }
    if now_unix >= claims.exp {
        return false;
    }
    let share = share_bp(expected_instance_id);
    if age_reached(claims.iat, claims.exp, share, now_unix) {
        return false;
    }
    let jwks = jwks_fetch(relay_base);
    match jwks {
        JwksSet::Healthy { kids } => kids.iter().any(|k| k == &new_kid),
        JwksSet::Unhealthy => false,
    }
}

pub fn evaluate_once(config: &RenewalConfig, state: &mut RenewalState) -> RenewalStep {
    let Ok(root) = JournalRoot::open(&config.journal_root).map(Arc::new) else {
        return record_skip(state, SkipClass::RootUnavailable);
    };
    evaluate_once_unless_cancelled(&root, config, state, &|| false)
}

fn evaluate_once_unless_cancelled(
    root: &Arc<JournalRoot>,
    config: &RenewalConfig,
    state: &mut RenewalState,
    cancelled: &dyn Fn() -> bool,
) -> RenewalStep {
    if cancelled() || root.revalidate_canonical_binding().is_err() {
        return RenewalStep::Current;
    }
    // 1. Skips (no HTTP)
    let is_spl = read_journal_config(&config.journal_root)
        .ok()
        .and_then(|r| r.config)
        .and_then(|c| c.get("link").cloned())
        .and_then(|l| l.get("posture").and_then(Value::as_str).map(str::to_owned))
        .as_deref()
        == Some("spl");
    if !is_spl {
        return record_skip(state, SkipClass::Posture);
    }

    let token = match load_link_service_token(&config.journal_root) {
        LinkServiceTokenRead::Present(t) => t,
        LinkServiceTokenRead::Missing => return record_skip(state, SkipClass::TokenMissing),
        LinkServiceTokenRead::Unreadable => return record_skip(state, SkipClass::TokenUnreadable),
        LinkServiceTokenRead::Malformed => return record_skip(state, SkipClass::TokenMalformed),
    };

    let identity = match load_link_state(&config.journal_root) {
        LinkStateRead::Present(id) => id,
        LinkStateRead::Missing => return record_skip(state, SkipClass::IdentityAbsent),
        LinkStateRead::Unreadable => return record_skip(state, SkipClass::IdentityUnreadable),
        LinkStateRead::Malformed => return record_skip(state, SkipClass::IdentityMalformed),
    };

    let ca = match load_service_identity_ca(&config.journal_root) {
        Ok(ca) => ca,
        Err(ServiceIdentityError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return record_skip(state, SkipClass::CaAbsent);
        }
        Err(_) => return record_skip(state, SkipClass::CaUnloadable),
    };

    let jid = match jid_from_spki(ca.spki_der()) {
        Ok(jid) => jid,
        Err(_) => return record_skip(state, SkipClass::CaUnloadable),
    };
    if jid != identity.instance_id {
        return record_skip(state, SkipClass::InstanceMismatch);
    }

    state.last_log_class = None;

    // 2. Block check (now_mono)
    let now_mono = (config.clock.now_mono)();
    if now_mono < state.backoff_deadline || now_mono < state.floor_end {
        return RenewalStep::Blocked;
    }

    // 3. Triggers check (now_unix)
    let now_unix = (config.clock.now_unix)();
    let claims_opt = spl_core::jwt::decode_unverified_claims(token.as_str())
        .filter(|_| parse_token_metadata(token.as_str()).kid.is_some());
    let trigger = match claims_opt {
        None => Some(Trigger::Undecodable),
        Some(claims) => {
            if now_unix >= claims.exp {
                Some(Trigger::Expired)
            } else {
                let share = share_bp(&identity.instance_id);
                let meta = parse_token_metadata(token.as_str());
                if age_reached(claims.iat, claims.exp, share, now_unix) {
                    Some(Trigger::Age)
                } else {
                    let health = (config.health_read)();
                    let refused = if service_token_health_is_refused(&health) {
                        if let Some(token_kid) = meta.kid.as_deref() {
                            let relay_base = relay_url(&config.journal_root);
                            let jwks = (config.jwks_fetch)(&relay_base);
                            match jwks {
                                JwksSet::Healthy { kids } => {
                                    if !kids.iter().any(|k| k == token_kid) {
                                        Some(Trigger::UnknownKey)
                                    } else if meta.pop {
                                        // A proven home's refused token was
                                        // replaced by a newer one it never
                                        // stored; only a signed call recovers.
                                        Some(Trigger::Refused)
                                    } else {
                                        None
                                    }
                                }
                                JwksSet::Unhealthy => None,
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    // Otherwise prove possession once. Against a relay that
                    // records no proofs the renewed token still lacks `pop`,
                    // and the post-success floor bounds this to a call a day.
                    refused.or((!meta.pop).then_some(Trigger::Unproven))
                }
            }
        }
    };

    let Some(trigger) = trigger else {
        return RenewalStep::Current;
    };

    // 4. Trigger fired: perform enrollment
    if cancelled() {
        return RenewalStep::Current;
    }
    let relay_base = relay_url(&config.journal_root);
    let pem = ca.public_key_spki_pem();
    let enroll_res = (config.enroll)(&relay_base, &identity.instance_id, &pem, EnrollMode::Renew);
    if cancelled() {
        return record_attempt(state, config, now_mono, trigger, Outcome::Superseded);
    }

    let outcome = match enroll_res {
        Err(EnrollError::Rejected {
            status: 404,
            reason: Some(r),
        }) if r == "not_enrolled" => Outcome::NotEnrolled,
        Err(EnrollError::Rejected { status: 409, .. }) => Outcome::Rejected { status: 409 },
        Err(EnrollError::Rejected { status, .. }) => Outcome::Rejected { status },
        Err(EnrollError::Unreachable(_)) => Outcome::Unreachable,
        Err(EnrollError::Response(_)) => Outcome::InvalidResponse,
        Err(EnrollError::Proof(_)) => Outcome::ProofUnavailable,
        Ok(new_token) => {
            let is_valid = validate_new_token(
                &new_token,
                &identity.instance_id,
                now_unix,
                &config.jwks_fetch,
                &relay_base,
            );
            if !is_valid {
                Outcome::InvalidResponse
            } else {
                (config.before_commit)();
                mutate_service_configuration(&config.journal_root, || {
                    if cancelled() {
                        return Outcome::Superseded;
                    }
                    let current_tok = match load_link_service_token(&config.journal_root) {
                        LinkServiceTokenRead::Present(t) => t,
                        _ => return Outcome::Superseded,
                    };
                    if current_tok.as_str() != token.as_str() {
                        return Outcome::Superseded;
                    }
                    let is_spl = read_journal_config(&config.journal_root)
                        .ok()
                        .and_then(|r| r.config)
                        .and_then(|c| c.get("link").cloned())
                        .and_then(|l| l.get("posture").and_then(Value::as_str).map(str::to_owned))
                        .as_deref()
                        == Some("spl");
                    if !is_spl {
                        return Outcome::Superseded;
                    }
                    if relay_url(&config.journal_root) != relay_base {
                        return Outcome::Superseded;
                    }
                    // This check admits publication. A later pause drains this
                    // already-admitted atomic save before restore can write.
                    if cancelled() {
                        return Outcome::Superseded;
                    }
                    match save_service_token_at_root_unlocked(root, &new_token, &|| {
                        if cancelled() {
                            Err("renewal interrupted".to_owned())
                        } else {
                            Ok(())
                        }
                    }) {
                        Ok(publication) if publication.confirmed() => Outcome::Renewed,
                        Ok(publication) => {
                            log::warn!(
                                "private-link renewal token was published with uncertainty: {publication:?}"
                            );
                            Outcome::PublishedUnconfirmed
                        }
                        Err(_) => Outcome::WriteFailed,
                    }
                })
            }
        }
    };

    record_attempt(state, config, now_mono, trigger, outcome)
}

fn record_skip(state: &mut RenewalState, class: SkipClass) -> RenewalStep {
    if state.last_log_class != Some(class) {
        if class == SkipClass::Posture {
            log::debug!(
                "private-link service token renewal skipped class={}",
                class.as_str()
            );
        } else {
            log::warn!(
                "private-link service token renewal skipped class={}",
                class.as_str()
            );
        }
        state.last_log_class = Some(class);
    }
    RenewalStep::Skipped { class }
}

fn record_attempt(
    state: &mut RenewalState,
    config: &RenewalConfig,
    now_mono: Duration,
    trigger: Trigger,
    outcome: Outcome,
) -> RenewalStep {
    let outcome_str = outcome.as_str();
    match &outcome {
        Outcome::Renewed | Outcome::PublishedUnconfirmed => {
            state.consecutive_ordinary_failures = 0;
            state.backoff_deadline = Duration::ZERO;
            state.floor_end = now_mono + Duration::from_secs(24 * 3600);
            log::info!(
                "private-link service token renewal trigger={} outcome={}",
                trigger.as_str(),
                outcome_str
            );
            state.last_log_outcome_str = Some(outcome_str);
        }
        Outcome::NotEnrolled | Outcome::Rejected { status: 409 } => {
            let wait = (config.jitter)(Duration::from_secs(6 * 3600));
            state.backoff_deadline = now_mono + wait;
            if state.last_log_outcome_str.as_deref() != Some(&outcome_str) {
                log::warn!(
                    "private-link service token renewal trigger={} outcome={}",
                    trigger.as_str(),
                    outcome_str
                );
            }
            state.last_log_outcome_str = Some(outcome_str);
        }
        Outcome::Superseded => {
            log::warn!(
                "private-link service token renewal trigger={} outcome={}",
                trigger.as_str(),
                outcome_str
            );
            state.last_log_outcome_str = Some(outcome_str);
        }
        Outcome::Unreachable
        | Outcome::Rejected { .. }
        | Outcome::InvalidResponse
        | Outcome::WriteFailed
        | Outcome::ProofUnavailable => {
            state.consecutive_ordinary_failures += 1;
            let k = state.consecutive_ordinary_failures;
            let base_mins = if k <= 1 {
                1u64
            } else if k >= 10 {
                360u64
            } else {
                (1u64 << (k - 1)).min(360)
            };
            let wait = (config.jitter)(Duration::from_secs(base_mins * 60));
            state.backoff_deadline = now_mono + wait;
            log::warn!(
                "private-link service token renewal trigger={} outcome={}",
                trigger.as_str(),
                outcome_str
            );
            state.last_log_outcome_str = Some(outcome_str);
        }
    }
    RenewalStep::Attempted { trigger, outcome }
}

pub fn spawn_service_token_renewal(config: RenewalConfig) -> tokio::task::JoinHandle<()> {
    let root = JournalRoot::open(&config.journal_root).map(Arc::new);
    tokio::spawn(async move {
        let Ok(root) = root else {
            log::warn!("private-link renewal root could not be retained");
            return;
        };
        let mut state = RenewalState::default();
        loop {
            tokio::time::sleep(config.tick).await;
            let config_clone = config.clone();
            let state_clone = state.clone();
            let evaluation_root = Arc::clone(&root);
            match tokio::task::spawn_blocking(move || {
                let mut s = state_clone;
                let step = evaluate_once_unless_cancelled(
                    &evaluation_root,
                    &config_clone,
                    &mut s,
                    &|| false,
                );
                (step, s)
            })
            .await
            {
                Ok((_, new_state)) => state = new_state,
                Err(_) => {
                    // An evaluation that did not finish may already have called the
                    // relay, so wait the longest backoff before trying again.
                    let wait = (config.jitter)(Duration::from_secs(6 * 3600));
                    state.backoff_deadline = (config.clock.now_mono)() + wait;
                    log::warn!("private-link service token renewal evaluation did not complete");
                }
            }
        }
    })
}

/// App-owned renewal lifecycle. Recovery drains admitted work; shutdown joins
/// active HTTP and publication. The original root remains bound across retries.
pub async fn run_embedded_service_token_renewal(
    root: Arc<JournalRoot>,
    mut config: RenewalConfig,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    config.journal_root = root.canonical_path().to_owned();
    let mut state = RenewalState::default();
    let mut observed_generation = None;
    loop {
        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(config.tick) => {},
            _ = shutdown.changed() => return,
        }
        // Admission performs filesystem checks off the runtime thread. Keep
        // its guard outside the evaluation so even a panic joins before drain.
        let admission_root = Arc::clone(&root);
        let (admission, generation) = match tokio::task::spawn_blocking(move || {
            let admission = begin_target_use_bound(Arc::clone(&admission_root))?;
            let generation = whole_journal_restore_generation(admission_root.canonical_path());
            Ok::<_, solstone_core_journal_io::whole_journal_restore::TargetUseRefused>((
                admission, generation,
            ))
        })
        .await
        {
            Ok(Ok(admitted)) => admitted,
            Ok(Err(_)) => continue,
            Err(_) => {
                log::warn!("private-link service token renewal admission did not complete");
                continue;
            }
        };
        if observed_generation != Some(generation) {
            state = RenewalState::default();
            observed_generation = Some(generation);
        }
        let config_clone = config.clone();
        let state_clone = state.clone();
        let stop = shutdown.clone();
        let evaluation_root = Arc::clone(&root);
        // Deliberately join this work even if shutdown arrives during HTTP.
        match tokio::task::spawn_blocking(move || {
            let mut state = state_clone;
            let cancelled = || {
                *stop.borrow()
                    || stop.has_changed().is_err()
                    || evaluation_root.revalidate_canonical_binding().is_err()
                    || whole_journal_restore_blocks(evaluation_root.canonical_path())
                    || whole_journal_restore_generation(evaluation_root.canonical_path())
                        != generation
            };
            evaluate_once_unless_cancelled(&evaluation_root, &config_clone, &mut state, &cancelled);
            state
        })
        .await
        {
            Ok(new_state) => state = new_state,
            Err(_) => {
                state.backoff_deadline =
                    (config.clock.now_mono)() + (config.jitter)(Duration::from_secs(6 * 3600));
                log::warn!("private-link service token renewal evaluation did not complete");
            }
        }
        drop(admission);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    use solstone_core_sol_link::ca::{generate_ca, jid_from_spki};
    use solstone_core_sol_link::service_identity::load_or_create_service_identity;

    use crate::link_state_files::LinkServiceTokenRead;
    use crate::private_link::{enable_spl_with, save_service_token};

    struct TempJournal(PathBuf);

    impl TempJournal {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-spl-renewal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn base64_url_encode(bytes: &[u8]) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut b = (chunk[0] as u32) << 16;
            if chunk.len() > 1 {
                b |= (chunk[1] as u32) << 8;
            }
            if chunk.len() > 2 {
                b |= chunk[2] as u32;
            }
            out.push(CHARS[((b >> 18) & 0x3F) as usize] as char);
            out.push(CHARS[((b >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(CHARS[((b >> 6) & 0x3F) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(CHARS[(b & 0x3F) as usize] as char);
            }
        }
        out
    }

    fn make_test_jwt(
        kid: Option<&str>,
        instance_id: &str,
        iat: i64,
        exp: i64,
        scope: &str,
        sub: Option<&str>,
    ) -> String {
        let header = match kid {
            Some(k) => {
                let h = serde_json::json!({"alg": "none", "typ": "JWT", "kid": k});
                base64_url_encode(serde_json::to_string(&h).unwrap().as_bytes())
            }
            None => "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0".to_owned(),
        };
        let sub_str = sub
            .map(|s| s.to_owned())
            .unwrap_or_else(|| format!("home:{instance_id}"));
        let payload = serde_json::json!({
            "iss": "sol-relay",
            "sub": sub_str,
            "aud": "relay",
            "scope": scope,
            "instance_id": instance_id,
            "iat": iat,
            "exp": exp,
            "jti": "jwt-1",
            "pop": true,
        });
        let payload_b64 = base64_url_encode(serde_json::to_string(&payload).unwrap().as_bytes());
        format!("{header}.{payload_b64}.sig")
    }

    /// A token from before this home proved possession: no `pop` claim.
    fn make_unproven_test_jwt(kid: &str, instance_id: &str, iat: i64, exp: i64) -> String {
        let proven = make_test_jwt(Some(kid), instance_id, iat, exp, "session.listen", None);
        let (header, rest) = proven.split_once('.').unwrap();
        let (payload, signature) = rest.split_once('.').unwrap();
        let mut claims: serde_json::Value =
            serde_json::from_slice(&decode_base64url(payload).unwrap()).unwrap();
        claims.as_object_mut().unwrap().remove("pop");
        let payload = base64_url_encode(serde_json::to_string(&claims).unwrap().as_bytes());
        format!("{header}.{payload}.{signature}")
    }

    fn write_test_journal_config(journal_path: &Path, posture: &str, relay_url: Option<&str>) {
        let config_dir = journal_path.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let mut link = serde_json::Map::new();
        link.insert(
            "posture".to_owned(),
            serde_json::Value::String(posture.to_owned()),
        );
        if let Some(url) = relay_url {
            link.insert(
                "relay_url".to_owned(),
                serde_json::Value::String(url.to_owned()),
            );
        }
        let mut root = serde_json::Map::new();
        root.insert("link".to_owned(), serde_json::Value::Object(link));
        std::fs::write(
            config_dir.join("journal.json"),
            serde_json::to_string(&root).unwrap(),
        )
        .unwrap();
    }

    fn setup_test_journal(journal: &TempJournal, token_str: &str) -> String {
        write_test_journal_config(journal.path(), "spl", Some("https://relay.example.com"));
        let identity = load_or_create_service_identity(journal.path()).unwrap();
        save_service_token(journal.path(), token_str).unwrap();
        identity.instance_id
    }

    #[cfg(all(unix, feature = "test-hooks"))]
    #[test]
    fn actual_uncertain_renewal_publication_keeps_the_existing_floor() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };
        let journal = TempJournal::new();
        let id = setup_test_journal(&journal, "expired-token");
        let token = make_test_jwt(
            Some("current"),
            &id,
            1_000_000,
            1_000_000 + 365 * 86_400,
            "session.listen",
            None,
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let enrolled = Arc::clone(&calls);
        let config = RenewalConfig {
            journal_root: journal.path().to_owned(),
            enroll: Arc::new(move |_, _, _, mode| {
                assert_eq!(mode, EnrollMode::Renew);
                enrolled.fetch_add(1, Ordering::SeqCst);
                Ok(token.clone())
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["current".to_owned()],
            }),
            health_read: Arc::new(|| ServiceTokenHealth {
                event_at_ms: None,
                now_ms: 0,
                reason: None,
                tunnel_error_at_ms: None,
                tunnel_success_at_ms: None,
            }),
            clock: RenewalClock {
                now_unix: Arc::new(|| 1_000_100),
                now_mono: Arc::new(|| Duration::from_secs(100)),
            },
            tick: Duration::from_secs(1),
            jitter: Arc::new(|duration| duration),
            before_commit: Arc::new(|| {}),
        };
        let mut state = RenewalState::default();
        let (step, consumed) =
            run_with_bound_publication_fault(BoundPublicationPrimitive::ParentSync, 1, 5, || {
                evaluate_once(&config, &mut state)
            });
        assert!(consumed);
        assert!(matches!(
            step,
            RenewalStep::Attempted {
                outcome: Outcome::PublishedUnconfirmed,
                ..
            }
        ));
        assert_eq!(state.floor_end, Duration::from_secs(100 + 24 * 3600));
        assert_eq!(state.consecutive_ordinary_failures, 0);
        // Even if a later read sees an undecodable token, uncertainty must not
        // be treated as a failed write and immediately repeat enrollment.
        save_service_token(journal.path(), "undecodable-after-publication").unwrap();
        assert_eq!(evaluate_once(&config, &mut state), RenewalStep::Blocked);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn replacement_after_final_evaluation_check_cannot_receive_the_renewed_token() {
        let journal = TempJournal::new();
        let id = setup_test_journal(&journal, "expired-token");
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let token = make_test_jwt(
            Some("current"),
            &id,
            1_000_000,
            1_000_000 + 365 * 86_400,
            "session.listen",
            None,
        );
        let armed = Arc::new(AtomicBool::new(false));
        let arm = Arc::clone(&armed);
        let config = RenewalConfig {
            journal_root: journal.path().to_owned(),
            enroll: Arc::new(move |_, _, _, _| Ok(token.clone())),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["current".to_owned()],
            }),
            health_read: Arc::new(|| ServiceTokenHealth {
                event_at_ms: None,
                now_ms: 0,
                reason: None,
                tunnel_error_at_ms: None,
                tunnel_success_at_ms: None,
            }),
            clock: RenewalClock {
                now_unix: Arc::new(|| 1_000_100),
                now_mono: Arc::new(|| Duration::from_secs(100)),
            },
            tick: Duration::from_secs(1),
            jitter: Arc::new(|duration| duration),
            before_commit: Arc::new(move || {
                arm.store(true, Ordering::SeqCst);
            }),
        };
        let checks = std::cell::Cell::new(0);
        let retained = journal.path().with_extension("retained");
        let cancelled = || {
            if armed.load(Ordering::SeqCst) {
                let previous = checks.replace(checks.get() + 1);
                if previous == 1 {
                    // Exactly the final evaluation check: admit it, then
                    // substitute the pathname before the writer opens it.
                    std::fs::rename(journal.path(), &retained).unwrap();
                    std::fs::create_dir_all(journal.path().join("link/tokens")).unwrap();
                    std::fs::write(
                        journal.path().join("link/tokens/account.json"),
                        b"replacement-control",
                    )
                    .unwrap();
                }
            }
            false
        };
        let mut state = RenewalState::default();
        let step = evaluate_once_unless_cancelled(&root, &config, &mut state, &cancelled);
        let actual = std::fs::read(journal.path().join("link/tokens/account.json")).unwrap();
        let original = std::fs::read(retained.join("link/tokens/account.json")).unwrap();
        std::fs::remove_dir_all(retained).unwrap();
        assert!(
            checks.get() >= 2,
            "the final evaluation check must be reached"
        );
        assert_eq!(
            actual, b"replacement-control",
            "replacement journal must receive no credential"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&original).unwrap()["service_token"],
            "expired-token"
        );
        assert!(matches!(
            step,
            RenewalStep::Attempted {
                outcome: Outcome::WriteFailed,
                ..
            }
        ));
    }

    #[test]
    fn age_share_is_stable_and_inclusive() {
        let zero_uuid = "00000000-0000-0000-0000-000000000000";
        assert_eq!(share_bp(zero_uuid), 8306);
        let digest = Sha256::digest(zero_uuid.as_bytes());
        let mut hi64_bytes = [0u8; 8];
        hi64_bytes.copy_from_slice(&digest[..8]);
        let hi64 = u64::from_be_bytes(hi64_bytes);
        assert_eq!(8306, (7500 + (hi64 % 1001)) as u16);

        // 365-day token
        let iat = 1_000_000i64;
        let exp = iat + 365 * 86_400;
        let lifetime = exp - iat;
        let offset = (lifetime * 8306) / 10_000;
        let threshold = iat + offset;
        assert!(!age_reached(iat, exp, 8306, threshold - 1));
        assert!(age_reached(iat, exp, 8306, threshold));
        assert!(age_reached(iat, exp, 8306, threshold + 1));

        // 100 ids share-{i:03}
        let mut shares = Vec::new();
        for i in 0..100 {
            let s = share_bp(&format!("share-{i:03}"));
            assert!((7500..=8500).contains(&s));
            shares.push(s);
        }
        let max = *shares.iter().max().unwrap();
        let min = *shares.iter().min().unwrap();
        assert!(max - min >= 500, "max - min was {}", max - min);

        // 10-day token renews at its share of 10 days
        let exp_10d = iat + 10 * 86_400;
        let offset_10d = (10 * 86_400 * 8306) / 10_000;
        let threshold_10d = iat + offset_10d;
        assert!(!age_reached(iat, exp_10d, 8306, threshold_10d - 1));
        assert!(age_reached(iat, exp_10d, 8306, threshold_10d));

        // exp <= iat, future iat, and overflows
        assert!(!age_reached(100, 100, 8000, 150));
        assert!(!age_reached(100, 50, 8000, 150));
        assert!(!age_reached(200, 300, 8000, 100));
        assert!(!age_reached(0, i64::MAX, 8000, 100));
        assert!(!age_reached(i64::MAX - 5, i64::MAX, 8000, 100));
    }

    #[test]
    fn unknown_key_requires_a_healthy_set_that_lacks_the_kid() {
        // classify_jwks byte cases
        let ok = classify_jwks(200, br#"{"keys":[{"kid":"kid-test-1"}]}"#);
        assert_eq!(
            ok,
            JwksSet::Healthy {
                kids: vec!["kid-test-1".into()]
            }
        );
        assert_eq!(classify_jwks(503, br#"{"keys":[]}"#), JwksSet::Unhealthy);
        assert_eq!(classify_jwks(200, br#"{"keys":[]}"#), JwksSet::Unhealthy);
        assert_eq!(classify_jwks(200, b"not json"), JwksSet::Unhealthy);
        assert_eq!(classify_jwks(200, br#"{"keys":"abc"}"#), JwksSet::Unhealthy);
        assert_eq!(
            classify_jwks(200, br#"{"keys":[{"kid":123}]}"#),
            JwksSet::Unhealthy
        );
        let mut large = br#"{"keys":[{"kid":"kid-test-2"}],"pad":""#.to_vec();
        large.resize(64 * 1024 + 1, b'a');
        large.extend_from_slice(br#""}"#);
        assert!(serde_json::from_slice::<Value>(&large).is_ok());
        assert_eq!(classify_jwks(200, &large), JwksSet::Unhealthy);

        // evaluate_once with young token and refused health
        let journal = TempJournal::new();
        let iat = 1_000_000i64;
        let exp = iat + 365 * 86_400;
        let tok = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            iat,
            exp,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &tok);
        let tok = make_test_jwt(
            Some("kid-test-1"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );
        save_service_token(journal.path(), &tok).unwrap();

        let new_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );

        let test_with_jwks = |jwks: JwksSet| {
            let jwks_cell = Arc::new(Mutex::new(jwks));
            let mut state = RenewalState::default();
            let cfg = RenewalConfig {
                journal_root: journal.path().to_path_buf(),
                enroll: Arc::new({
                    let new_tok = new_tok.clone();
                    move |_, _, _, _| Ok(new_tok.clone())
                }),
                jwks_fetch: Arc::new({
                    let jwks_cell = jwks_cell.clone();
                    move |_| jwks_cell.lock().unwrap().clone()
                }),
                health_read: Arc::new(|| ServiceTokenHealth {
                    event_at_ms: Some(100_000),
                    now_ms: 100_000,
                    reason: Some("service_token_rejected".into()),
                    tunnel_error_at_ms: Some(100),
                    tunnel_success_at_ms: Some(50),
                }),
                clock: RenewalClock {
                    now_unix: Arc::new(move || iat + 100),
                    now_mono: Arc::new(|| Duration::from_secs(1000)),
                },
                tick: Duration::from_secs(60),
                jitter: Arc::new(|d| d),
                before_commit: Arc::new(|| {}),
            };
            evaluate_once(&cfg, &mut state)
        };

        // 200 with the kid: a proven token the relay refuses was replaced by
        // one this home never stored, so it renews with a signed call.
        assert!(matches!(
            test_with_jwks(JwksSet::Healthy {
                kids: vec!["kid-test-1".into()]
            }),
            RenewalStep::Attempted {
                trigger: Trigger::Refused,
                ..
            }
        ));

        // 200 with other kid (renew) -> Attempted
        assert!(matches!(
            test_with_jwks(JwksSet::Healthy {
                kids: vec!["kid-test-2".into()]
            }),
            RenewalStep::Attempted {
                trigger: Trigger::UnknownKey,
                outcome: Outcome::Renewed
            }
        ));

        // Unhealthy cases -> Current
        assert_eq!(test_with_jwks(JwksSet::Unhealthy), RenewalStep::Current);
    }

    #[test]
    fn not_refused_health_skips_jwks_fetch_and_enrollment() {
        let journal = TempJournal::new();
        let iat = 1_000_000i64;
        let exp = iat + 365 * 86_400;
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            iat,
            exp,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);
        let tok = make_test_jwt(
            Some("kid-test-1"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );
        save_service_token(journal.path(), &tok).unwrap();

        let run_case = |health: ServiceTokenHealth| {
            let fetch_count = Arc::new(AtomicUsize::new(0));
            let enroll_count = Arc::new(AtomicUsize::new(0));
            let mut state = RenewalState::default();
            let cfg = RenewalConfig {
                journal_root: journal.path().to_path_buf(),
                enroll: Arc::new({
                    let enroll_count = enroll_count.clone();
                    move |_, _, _, _| {
                        enroll_count.fetch_add(1, Ordering::SeqCst);
                        Ok("unused".into())
                    }
                }),
                jwks_fetch: Arc::new({
                    let fetch_count = fetch_count.clone();
                    move |_| {
                        fetch_count.fetch_add(1, Ordering::SeqCst);
                        JwksSet::Healthy {
                            kids: vec!["kid-test-1".into()],
                        }
                    }
                }),
                health_read: Arc::new(move || health.clone()),
                clock: RenewalClock {
                    now_unix: Arc::new(move || iat + 100),
                    now_mono: Arc::new(|| Duration::from_secs(1000)),
                },
                tick: Duration::from_secs(60),
                jitter: Arc::new(|d| d),
                before_commit: Arc::new(|| {}),
            };
            let step = evaluate_once(&cfg, &mut state);
            (
                step,
                fetch_count.load(Ordering::SeqCst),
                enroll_count.load(Ordering::SeqCst),
            )
        };

        // 1. No payload
        let (step, f, e) = run_case(ServiceTokenHealth {
            event_at_ms: None,
            now_ms: 100_000,
            reason: Some("service_token_rejected".into()),
            tunnel_error_at_ms: Some(100),
            tunnel_success_at_ms: Some(50),
        });
        assert_eq!(step, RenewalStep::Current);
        assert_eq!(f, 0);
        assert_eq!(e, 0);

        // 2. Event older than 90s
        let (step, f, e) = run_case(ServiceTokenHealth {
            event_at_ms: Some(10_000),
            now_ms: 100_001 + 10_000,
            reason: Some("service_token_rejected".into()),
            tunnel_error_at_ms: Some(100),
            tunnel_success_at_ms: Some(50),
        });
        assert_eq!(step, RenewalStep::Current);
        assert_eq!(f, 0);
        assert_eq!(e, 0);

        // 3. Error timestamp before last success
        let (step, f, e) = run_case(ServiceTokenHealth {
            event_at_ms: Some(100_000),
            now_ms: 100_000,
            reason: Some("service_token_rejected".into()),
            tunnel_error_at_ms: Some(50),
            tunnel_success_at_ms: Some(100),
        });
        assert_eq!(step, RenewalStep::Current);
        assert_eq!(f, 0);
        assert_eq!(e, 0);

        // 4. relay_tunnel_rejected
        let (step, f, e) = run_case(ServiceTokenHealth {
            event_at_ms: Some(100_000),
            now_ms: 100_000,
            reason: Some("relay_tunnel_rejected".into()),
            tunnel_error_at_ms: Some(100),
            tunnel_success_at_ms: Some(50),
        });
        assert_eq!(step, RenewalStep::Current);
        assert_eq!(f, 0);
        assert_eq!(e, 0);

        // Fresh current service_token_rejected of a proven token whose set
        // contains kid: exactly 1 fetch, then a signed renewal.
        let (step, f, e) = run_case(ServiceTokenHealth {
            event_at_ms: Some(100_000),
            now_ms: 100_000,
            reason: Some("service_token_rejected".into()),
            tunnel_error_at_ms: Some(100),
            tunnel_success_at_ms: Some(50),
        });
        assert!(matches!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Refused,
                ..
            }
        ));
        assert_eq!(f, 1);
        assert_eq!(e, 1);

        // Future event_at_ms counts as fresh
        let (step, f, e) = run_case(ServiceTokenHealth {
            event_at_ms: Some(200_000),
            now_ms: 100_000,
            reason: Some("service_token_rejected".into()),
            tunnel_error_at_ms: Some(100),
            tunnel_success_at_ms: Some(50),
        });
        assert!(matches!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Refused,
                ..
            }
        ));
        assert_eq!(f, 1);
        assert_eq!(e, 1);
    }

    #[test]
    fn an_unproven_token_renews_once_with_a_signed_call_and_then_waits_a_day() {
        let journal = TempJournal::new();
        let iat = 1_000_000i64;
        let exp = iat + 365 * 86_400;
        let dummy = make_unproven_test_jwt("kid-test-1", "inst-1", iat, exp);
        let inst_id = setup_test_journal(&journal, &dummy);
        save_service_token(
            journal.path(),
            &make_unproven_test_jwt("kid-test-1", &inst_id, iat, exp),
        )
        .unwrap();
        // A relay that records no proofs answers with another unproven token.
        let renewed = make_unproven_test_jwt("kid-test-1", &inst_id, iat + 100, exp + 100);
        let modes = Arc::new(Mutex::new(Vec::new()));
        let mono = Arc::new(Mutex::new(Duration::from_secs(1000)));
        let cfg = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let renewed = renewed.clone();
                let modes = modes.clone();
                move |_, _, _, mode| {
                    modes.lock().unwrap().push(mode);
                    Ok(renewed.clone())
                }
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-1".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(move || iat + 100),
                now_mono: Arc::new({
                    let mono = mono.clone();
                    move || *mono.lock().unwrap()
                }),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };
        let mut state = RenewalState::default();
        assert_eq!(
            evaluate_once(&cfg, &mut state),
            RenewalStep::Attempted {
                trigger: Trigger::Unproven,
                outcome: Outcome::Renewed
            }
        );
        assert_eq!(*modes.lock().unwrap(), vec![EnrollMode::Renew]);
        *mono.lock().unwrap() += Duration::from_secs(23 * 3600);
        assert_eq!(evaluate_once(&cfg, &mut state), RenewalStep::Blocked);
        *mono.lock().unwrap() += Duration::from_secs(2 * 3600);
        assert!(matches!(
            evaluate_once(&cfg, &mut state),
            RenewalStep::Attempted {
                trigger: Trigger::Unproven,
                ..
            }
        ));
        assert_eq!(modes.lock().unwrap().len(), 2);
    }

    #[test]
    fn expired_or_undecodable_token_renews_missing_or_malformed_skips() {
        let journal = TempJournal::new();
        let iat = 1_000_000i64;
        let exp = iat + 86_400;
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            iat,
            exp,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);

        let new_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            exp + 10,
            exp + 10 + 365 * 86_400,
            "session.listen",
            None,
        );

        let make_cfg = |now_unix_val: i64, fetch_count: Arc<AtomicUsize>| RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let new_tok = new_tok.clone();
                move |_, _, _, _| Ok(new_tok.clone())
            }),
            jwks_fetch: Arc::new({
                let fetch_count = fetch_count.clone();
                move |_| {
                    fetch_count.fetch_add(1, Ordering::SeqCst);
                    JwksSet::Healthy {
                        kids: vec!["kid-test-2".into()],
                    }
                }
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(move || now_unix_val),
                now_mono: Arc::new(|| Duration::from_secs(1000)),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        // 1. Expired token: renews, zero decision-time fetches, validation fetch happens (1 total)
        let tok = make_test_jwt(
            Some("kid-test-1"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );
        save_service_token(journal.path(), &tok).unwrap();
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(exp + 1, fetch_count.clone()), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Expired,
                outcome: Outcome::Renewed
            }
        );
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);

        // 2. Undecodable string: renews
        save_service_token(journal.path(), "undecodable-token").unwrap();
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(iat + 100, fetch_count.clone()), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Undecodable,
                outcome: Outcome::Renewed
            }
        );
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);

        // 2b. Decodable claims without a header key id: renews as undecodable
        let no_kid = make_test_jwt(None, &inst_id, iat, exp, "session.listen", None);
        save_service_token(journal.path(), &no_kid).unwrap();
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(iat + 100, fetch_count.clone()), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Undecodable,
                outcome: Outcome::Renewed
            }
        );

        // 3. Missing token: no enroll, no fetch, file untouched
        std::fs::remove_file(journal.path().join("link/tokens/account.json")).unwrap();
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(iat + 100, fetch_count.clone()), &mut state);
        assert_eq!(
            step,
            RenewalStep::Skipped {
                class: SkipClass::TokenMissing
            }
        );
        assert_eq!(fetch_count.load(Ordering::SeqCst), 0);
        assert!(!journal.path().join("link/tokens/account.json").exists());

        // 4. Malformed token: no enroll, no fetch, file untouched
        std::fs::write(
            journal.path().join("link/tokens/account.json"),
            b"{not json",
        )
        .unwrap();
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(iat + 100, fetch_count.clone()), &mut state);
        assert_eq!(
            step,
            RenewalStep::Skipped {
                class: SkipClass::TokenMalformed
            }
        );
        assert_eq!(fetch_count.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read(journal.path().join("link/tokens/account.json")).unwrap(),
            b"{not json"
        );
    }

    #[test]
    fn healthy_health_renews_only_when_age_threshold_is_reached() {
        let journal = TempJournal::new();
        let iat = 1_000_000i64;
        let exp = iat + 365 * 86_400;
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            iat,
            exp,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);
        let tok = make_test_jwt(
            Some("kid-test-1"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );
        save_service_token(journal.path(), &tok).unwrap();

        let share = share_bp(&inst_id);
        let offset = ((exp - iat) * share as i64) / 10_000;
        let threshold = iat + offset;

        let new_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            threshold + 10,
            threshold + 10 + 365 * 86_400,
            "session.listen",
            None,
        );

        let make_cfg = |now_unix_val: i64, fetch_count: Arc<AtomicUsize>| RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let new_tok = new_tok.clone();
                move |_, _, _, _| Ok(new_tok.clone())
            }),
            jwks_fetch: Arc::new({
                let fetch_count = fetch_count.clone();
                move |_| {
                    fetch_count.fetch_add(1, Ordering::SeqCst);
                    JwksSet::Healthy {
                        kids: vec!["kid-test-2".into()],
                    }
                }
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(move || now_unix_val),
                now_mono: Arc::new(|| Duration::from_secs(1000)),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        // Young token (before threshold): does not renew and does not fetch
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(threshold - 1, fetch_count.clone()), &mut state);
        assert_eq!(step, RenewalStep::Current);
        assert_eq!(fetch_count.load(Ordering::SeqCst), 0);

        // Over-age token (at threshold): renews with trigger Age
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(threshold, fetch_count.clone()), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Age,
                outcome: Outcome::Renewed
            }
        );
    }

    #[test]
    fn unmet_local_prerequisites_skip_renewal_without_mutations() {
        let journal = TempJournal::new();
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            100,
            200,
            "session.listen",
            None,
        );
        setup_test_journal(&journal, &dummy);

        let make_cfg = || RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new(|_, _, _, _| panic!("enroll must not be called")),
            jwks_fetch: Arc::new(|_| panic!("jwks_fetch must not be called")),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(|| 150),
                now_mono: Arc::new(|| Duration::from_secs(1000)),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        // 1. Posture not spl
        write_test_journal_config(journal.path(), "direct", None);
        let mut state = RenewalState::default();
        assert_eq!(
            evaluate_once(&make_cfg(), &mut state),
            RenewalStep::Skipped {
                class: SkipClass::Posture
            }
        );

        // Restore posture
        write_test_journal_config(journal.path(), "spl", Some("https://relay.example.com"));

        // 2. CA files absent
        let cert_bytes = std::fs::read(journal.path().join("link/ca/cert.pem")).unwrap();
        let key_bytes = std::fs::read(journal.path().join("link/ca/private.pem")).unwrap();
        std::fs::remove_file(journal.path().join("link/ca/cert.pem")).unwrap();
        std::fs::remove_file(journal.path().join("link/ca/private.pem")).unwrap();
        let mut state = RenewalState::default();
        assert_eq!(
            evaluate_once(&make_cfg(), &mut state),
            RenewalStep::Skipped {
                class: SkipClass::CaAbsent
            }
        );
        assert!(!journal.path().join("link/ca/cert.pem").exists());
        assert!(!journal.path().join("link/ca/private.pem").exists());

        // Restore CA
        std::fs::write(journal.path().join("link/ca/cert.pem"), &cert_bytes).unwrap();
        std::fs::write(journal.path().join("link/ca/private.pem"), &key_bytes).unwrap();

        // 3. Identity state absent
        let _state_bytes = std::fs::read(journal.path().join("link/state.json")).unwrap();
        std::fs::remove_file(journal.path().join("link/state.json")).unwrap();
        let mut state = RenewalState::default();
        assert_eq!(
            evaluate_once(&make_cfg(), &mut state),
            RenewalStep::Skipped {
                class: SkipClass::IdentityAbsent
            }
        );
        assert!(!journal.path().join("link/state.json").exists());

        // 4. Stored instance_id disagrees with jid_from_spki
        std::fs::write(
            journal.path().join("link/state.json"),
            serde_json::json!({"instance_id": "mismatched-id", "home_label": "lbl", "created_at": "2026-01-01T00:00:00Z"}).to_string(),
        )
        .unwrap();
        let mut state = RenewalState::default();
        assert_eq!(
            evaluate_once(&make_cfg(), &mut state),
            RenewalStep::Skipped {
                class: SkipClass::InstanceMismatch
            }
        );
        let current_state =
            std::fs::read_to_string(journal.path().join("link/state.json")).unwrap();
        assert!(current_state.contains("mismatched-id"));
    }

    #[test]
    fn renewal_passes_stored_identity_ca_pem_and_renew_mode() {
        let journal = TempJournal::new();
        let iat = 1_000_000i64;
        let exp = iat + 86_400;
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            iat,
            exp,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);
        let ca = load_service_identity_ca(journal.path()).unwrap();
        let expected_pem = ca.public_key_spki_pem();

        let recorded = Arc::new(Mutex::new(None));
        let new_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            exp + 10,
            exp + 10 + 365 * 86_400,
            "session.listen",
            None,
        );

        let cfg = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let recorded = recorded.clone();
                let new_tok = new_tok.clone();
                move |url, id, pem, mode| {
                    *recorded.lock().unwrap() =
                        Some((url.to_owned(), id.to_owned(), pem.to_owned(), mode));
                    Ok(new_tok.clone())
                }
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-2".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(move || exp + 1), // expired
                now_mono: Arc::new(|| Duration::from_secs(1000)),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        let mut state = RenewalState::default();
        let step = evaluate_once(&cfg, &mut state);
        assert!(matches!(
            step,
            RenewalStep::Attempted {
                outcome: Outcome::Renewed,
                ..
            }
        ));

        let (url, id, pem, mode) = recorded.lock().unwrap().take().expect("enroll recorded");
        assert_eq!(url, "https://relay.example.com");
        assert_eq!(id, inst_id);
        assert_eq!(pem, expected_pem);
        assert_eq!(mode, EnrollMode::Renew);
    }

    #[test]
    fn invalid_enrollment_responses_leave_token_intact_and_trigger_backoff() {
        let ca_low_share = loop {
            let ca = generate_ca().unwrap();
            let jid = jid_from_spki(ca.spki_der()).unwrap();
            if (share_bp(&jid) as i64) * 365 < 300 * 10_000 {
                break ca;
            }
        };

        let journal = TempJournal::new();
        let ca_dir = journal.path().join("link/ca");
        std::fs::create_dir_all(&ca_dir).unwrap();
        std::fs::write(ca_dir.join("cert.pem"), ca_low_share.certificate_pem()).unwrap();
        std::fs::write(ca_dir.join("private.pem"), ca_low_share.private_key_pem()).unwrap();
        let inst_id = jid_from_spki(ca_low_share.spki_der()).unwrap();
        std::fs::create_dir_all(journal.path().join("link")).unwrap();
        std::fs::write(
            journal.path().join("link/state.json"),
            serde_json::json!({
                "instance_id": inst_id,
                "home_label": "test",
                "created_at": "2026-01-01T00:00:00Z"
            })
            .to_string(),
        )
        .unwrap();
        write_test_journal_config(journal.path(), "spl", Some("https://relay.example.com"));

        let old_token_str = "original-expired-token";
        save_service_token(journal.path(), old_token_str).unwrap();

        let run_invalid_case = |token_result: String, jwks: JwksSet, now_unix_val: i64| {
            let mut state = RenewalState::default();
            let cfg = RenewalConfig {
                journal_root: journal.path().to_path_buf(),
                enroll: Arc::new({
                    let token_result = token_result.clone();
                    move |_, _, _, _| Ok(token_result.clone())
                }),
                jwks_fetch: Arc::new(move |_| jwks.clone()),
                health_read: Arc::new(ServiceTokenHealth::default),
                clock: RenewalClock {
                    now_unix: Arc::new(move || now_unix_val),
                    now_mono: Arc::new(|| Duration::from_secs(1000)),
                },
                tick: Duration::from_secs(60),
                jitter: Arc::new(|d| d),
                before_commit: Arc::new(|| {}),
            };

            let step = evaluate_once(&cfg, &mut state);
            assert_eq!(
                step,
                RenewalStep::Attempted {
                    trigger: Trigger::Undecodable,
                    outcome: Outcome::InvalidResponse
                }
            );

            // Token file byte-for-byte unchanged
            let cur_tok = crate::link_state_files::load_link_service_token(journal.path());
            match cur_tok {
                LinkServiceTokenRead::Present(t) => {
                    assert_eq!(t.as_str(), old_token_str);
                }
                _ => panic!("token not present"),
            }

            // Following evaluation is backed off
            let step2 = evaluate_once(&cfg, &mut state);
            assert_eq!(step2, RenewalStep::Blocked);
        };

        let now_unix = 1_000_000i64;
        let healthy_jwks = JwksSet::Healthy {
            kids: vec!["kid-test-2".into()],
        };

        // 1. Non-JWT
        run_invalid_case("not-a-jwt".into(), healthy_jwks.clone(), now_unix);

        // 2. exp <= iat
        let bad = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            100,
            100,
            "session.listen",
            None,
        );
        run_invalid_case(bad, healthy_jwks.clone(), now_unix);

        // 3. Already expired
        let bad = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            now_unix - 200,
            now_unix - 10,
            "session.listen",
            None,
        );
        run_invalid_case(bad, healthy_jwks.clone(), now_unix);

        // 4. 10-minute lifetime (< 86_400)
        let bad = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            now_unix,
            now_unix + 600,
            "session.listen",
            None,
        );
        run_invalid_case(bad, healthy_jwks.clone(), now_unix);

        // 5. Wrong instance_id
        let bad = make_test_jwt(
            Some("kid-test-2"),
            "wrong-inst",
            now_unix,
            now_unix + 365 * 86_400,
            "session.listen",
            None,
        );
        run_invalid_case(bad, healthy_jwks.clone(), now_unix);

        // 6. Wrong sub
        let bad = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            now_unix,
            now_unix + 365 * 86_400,
            "session.listen",
            Some("wrong-sub"),
        );
        run_invalid_case(bad, healthy_jwks.clone(), now_unix);

        // 7. Wrong scope
        let bad = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            now_unix,
            now_unix + 365 * 86_400,
            "wrong-scope",
            None,
        );
        run_invalid_case(bad, healthy_jwks.clone(), now_unix);

        // 8. New kid absent from post-enroll set
        let bad = make_test_jwt(
            Some("kid-test-1"),
            &inst_id,
            now_unix,
            now_unix + 365 * 86_400,
            "session.listen",
            None,
        );
        run_invalid_case(bad, healthy_jwks.clone(), now_unix);

        // 9. Post-enroll fetch unhealthy
        let good_jwt = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            now_unix,
            now_unix + 365 * 86_400,
            "session.listen",
            None,
        );
        run_invalid_case(good_jwt.clone(), JwksSet::Unhealthy, now_unix);

        // 10. Home clock 300 days ahead of iat on 365-day token whose share is below 300/365
        let iat_10 = 0i64;
        let exp_10 = 365 * 86_400;
        let tok_10 = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            iat_10,
            exp_10,
            "session.listen",
            None,
        );
        let clock_300d = 300 * 86_400;
        run_invalid_case(tok_10, healthy_jwks, clock_300d);
    }

    #[test]
    fn exponential_backoff_and_conflict_ladder_schedules() {
        let journal = TempJournal::new();
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            100,
            200,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);

        // 1. Zero jitter, enroll always Unreachable, 1440 evaluations (minutes 0..=1439)
        let mono_min = Arc::new(AtomicU64::new(0));
        let cfg = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new(|_, _, _, _| Err(EnrollError::Unreachable("fail".into()))),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-1".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(|| 500), // expired
                now_mono: Arc::new({
                    let mono_min = mono_min.clone();
                    move || Duration::from_secs(mono_min.load(Ordering::SeqCst) * 60)
                }),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        let mut state = RenewalState::default();
        let mut attempts = Vec::new();
        for minute in 0..=1439 {
            mono_min.store(minute, Ordering::SeqCst);
            let step = evaluate_once(&cfg, &mut state);
            if matches!(step, RenewalStep::Attempted { .. }) {
                attempts.push(minute);
            }
        }
        assert_eq!(
            attempts,
            vec![0, 1, 3, 7, 15, 31, 63, 127, 255, 511, 871, 1231]
        );
        assert_eq!(attempts.len(), 12);
        for w in attempts.windows(2) {
            assert!(w[1] - w[0] >= 1, "attempts closer than 60s");
        }

        // 2. Jitter pinned at 5/4
        let mono_min = Arc::new(AtomicU64::new(0));
        let cfg_jitter = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new(|_, _, _, _| Err(EnrollError::Unreachable("fail".into()))),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-1".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(|| 500),
                now_mono: Arc::new({
                    let mono_min = mono_min.clone();
                    move || Duration::from_secs(mono_min.load(Ordering::SeqCst) * 60)
                }),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d * 5 / 4),
            before_commit: Arc::new(|| {}),
        };

        let mut state_j = RenewalState::default();
        let mut attempts_j = Vec::new();
        for minute in 0..=1439 {
            mono_min.store(minute, Ordering::SeqCst);
            let step = evaluate_once(&cfg_jitter, &mut state_j);
            if matches!(step, RenewalStep::Attempted { .. }) {
                attempts_j.push(minute);
            }
        }
        assert!(
            (10..=12).contains(&attempts_j.len()),
            "attempts count: {}",
            attempts_j.len()
        );
        assert_eq!(attempts_j[0], 0);
        let max_gap_mins = (6 * 60 * 5) / 4;
        for w in attempts_j.windows(2) {
            assert!(w[1] - w[0] <= max_gap_mins + 1);
        }

        // 3. rejected 409: exactly 4 attempts at 0, 360, 720, 1080
        let mono_min = Arc::new(AtomicU64::new(0));
        let cfg_409 = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new(|_, _, _, _| {
                Err(EnrollError::Rejected {
                    status: 409,
                    reason: None,
                })
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-1".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(|| 500),
                now_mono: Arc::new({
                    let mono_min = mono_min.clone();
                    move || Duration::from_secs(mono_min.load(Ordering::SeqCst) * 60)
                }),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        let mut state_409 = RenewalState::default();
        let mut attempts_409 = Vec::new();
        for minute in 0..=1439 {
            mono_min.store(minute, Ordering::SeqCst);
            let step = evaluate_once(&cfg_409, &mut state_409);
            if matches!(step, RenewalStep::Attempted { .. }) {
                attempts_409.push(minute);
            }
        }
        assert_eq!(attempts_409, vec![0, 360, 720, 1080]);
        assert_eq!(attempts_409.len(), 4);

        // 4. After one success and the floor, next failure and one after it are 60s apart
        let new_tok = make_test_jwt(
            Some("kid-test-1"),
            &inst_id,
            100,
            100 + 365 * 86_400,
            "session.listen",
            None,
        );
        let enroll_succeed = Arc::new(AtomicBool::new(true));
        let mono_sec = Arc::new(AtomicU64::new(0));
        let cfg_restart = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let enroll_succeed = enroll_succeed.clone();
                let new_tok = new_tok.clone();
                move |_, _, _, _| {
                    if enroll_succeed.load(Ordering::SeqCst) {
                        Ok(new_tok.clone())
                    } else {
                        Err(EnrollError::Unreachable("fail".into()))
                    }
                }
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-1".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(|| 500), // expired initially
                now_mono: Arc::new({
                    let mono_sec = mono_sec.clone();
                    move || Duration::from_secs(mono_sec.load(Ordering::SeqCst))
                }),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        let mut state_restart = RenewalState::default();
        mono_sec.store(0, Ordering::SeqCst);
        let s1 = evaluate_once(&cfg_restart, &mut state_restart);
        assert!(matches!(
            s1,
            RenewalStep::Attempted {
                outcome: Outcome::Renewed,
                ..
            }
        ));

        enroll_succeed.store(false, Ordering::SeqCst);
        // Floor expires at 24h = 86_400 seconds
        mono_sec.store(86_400, Ordering::SeqCst);
        save_service_token(journal.path(), "expired-token").unwrap();
        let s2 = evaluate_once(&cfg_restart, &mut state_restart);
        assert!(matches!(
            s2,
            RenewalStep::Attempted {
                outcome: Outcome::Unreachable,
                ..
            }
        ));
        assert_eq!(state_restart.consecutive_ordinary_failures, 1);
        assert_eq!(
            state_restart.backoff_deadline,
            Duration::from_secs(86_400 + 60)
        );

        // Blocked at 86_459
        mono_sec.store(86_400 + 59, Ordering::SeqCst);
        assert_eq!(
            evaluate_once(&cfg_restart, &mut state_restart),
            RenewalStep::Blocked
        );

        // Next failure at 86_400 + 60
        mono_sec.store(86_400 + 60, Ordering::SeqCst);
        let s3 = evaluate_once(&cfg_restart, &mut state_restart);
        assert!(matches!(
            s3,
            RenewalStep::Attempted {
                outcome: Outcome::Unreachable,
                ..
            }
        ));
        assert_eq!(state_restart.consecutive_ordinary_failures, 2);
    }

    #[test]
    fn floor_allows_one_enrollment_per_day() {
        let journal = TempJournal::new();
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            100,
            200,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);

        let iat = 1_000_000i64;
        let exp = iat + 365 * 86_400;
        let initial_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );
        save_service_token(journal.path(), &initial_tok).unwrap();

        let pending_validation_kid: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let enroll_calls = Arc::new(AtomicUsize::new(0));
        let decision_fetches = Arc::new(AtomicUsize::new(0));
        let mono_min = Arc::new(AtomicU64::new(0));
        let return_invalid_enroll = Arc::new(AtomicBool::new(false));

        let renew_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );

        let cfg = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let enroll_calls = enroll_calls.clone();
                let pending_validation_kid = pending_validation_kid.clone();
                let return_invalid_enroll = return_invalid_enroll.clone();
                let renew_tok = renew_tok.clone();
                move |_, _, _, _| {
                    enroll_calls.fetch_add(1, Ordering::SeqCst);
                    if return_invalid_enroll.load(Ordering::SeqCst) {
                        Ok("not-a-valid-token".to_owned())
                    } else {
                        *pending_validation_kid.lock().unwrap() = Some("kid-test-2".to_owned());
                        Ok(renew_tok.clone())
                    }
                }
            }),
            jwks_fetch: Arc::new({
                let pending_validation_kid = pending_validation_kid.clone();
                let decision_fetches = decision_fetches.clone();
                move |_| {
                    if let Some(kid) = pending_validation_kid.lock().unwrap().take() {
                        JwksSet::Healthy { kids: vec![kid] }
                    } else {
                        decision_fetches.fetch_add(1, Ordering::SeqCst);
                        JwksSet::Healthy {
                            kids: vec!["kid-test-1".to_owned()],
                        }
                    }
                }
            }),
            health_read: Arc::new(|| ServiceTokenHealth {
                event_at_ms: Some(100_000),
                now_ms: 100_000,
                reason: Some("service_token_rejected".into()),
                tunnel_error_at_ms: Some(100),
                tunnel_success_at_ms: Some(50),
            }),
            clock: RenewalClock {
                now_unix: Arc::new(move || iat + 100),
                now_mono: Arc::new({
                    let mono_min = mono_min.clone();
                    move || Duration::from_secs(mono_min.load(Ordering::SeqCst) * 60)
                }),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        let mut state = RenewalState::default();

        // Minute-step evaluations for 2 days (0..2*1440): the second day shows
        // the floor re-arming after the enrollment at its boundary.
        let mut daily_enroll_minutes = Vec::new();
        for minute in 0..(2 * 1440) {
            mono_min.store(minute, Ordering::SeqCst);
            let step = evaluate_once(&cfg, &mut state);
            if matches!(
                step,
                RenewalStep::Attempted {
                    outcome: Outcome::Renewed,
                    ..
                }
            ) {
                daily_enroll_minutes.push(minute);
            }
        }
        // Enroll count in [0, 1440) is exactly 1
        let enrolls_in_first_day = daily_enroll_minutes.iter().filter(|&&m| m < 1440).count();
        assert_eq!(enrolls_in_first_day, 1);
        assert_eq!(daily_enroll_minutes, vec![0, 1440]);

        // Decision-time fetches do not increase during floor
        assert_eq!(decision_fetches.load(Ordering::SeqCst), 2);
        assert_eq!(enroll_calls.load(Ordering::SeqCst), 2);

        // At minute 1440 (floor ends), refused evaluation triggers UnknownKey
        // (proven by minute 1440 succeeding with trigger UnknownKey in daily_enroll_minutes)

        // One post-floor enroll returns non-token (InvalidResponse) and next tick inside 1-minute backoff does not fetch
        mono_min.store(2 * 1440, Ordering::SeqCst);
        return_invalid_enroll.store(true, Ordering::SeqCst);
        let dec_fetches_before = decision_fetches.load(Ordering::SeqCst);
        let step_fail = evaluate_once(&cfg, &mut state);
        assert_eq!(
            step_fail,
            RenewalStep::Attempted {
                trigger: Trigger::UnknownKey,
                outcome: Outcome::InvalidResponse,
            }
        );
        assert_eq!(
            decision_fetches.load(Ordering::SeqCst),
            dec_fetches_before + 1
        );

        // Next tick inside 1-minute backoff does not fetch
        let dec_fetches_blocked = decision_fetches.load(Ordering::SeqCst);
        let step_blocked = evaluate_once(&cfg, &mut state);
        assert_eq!(step_blocked, RenewalStep::Blocked);
        assert_eq!(decision_fetches.load(Ordering::SeqCst), dec_fetches_blocked);

        // Deleting floor (state.floor_end = Duration::ZERO) causes [0, 1440) enroll count to
        // exceed 1; its first hour already shows that.
        let mut state_no_floor = RenewalState::default();
        return_invalid_enroll.store(false, Ordering::SeqCst);
        let enrolls_before = enroll_calls.load(Ordering::SeqCst);
        for minute in 0..60 {
            mono_min.store(minute, Ordering::SeqCst);
            state_no_floor.floor_end = Duration::ZERO;
            let _ = evaluate_once(&cfg, &mut state_no_floor);
        }
        let enrolls_without_floor = enroll_calls.load(Ordering::SeqCst) - enrolls_before;
        assert!(
            enrolls_without_floor > 1,
            "expected > 1 enrolls without floor, got {}",
            enrolls_without_floor
        );
    }

    #[test]
    fn pre_boundary_hooks_detect_superseded_state_and_preserve_atomic_writes() {
        let journal = TempJournal::new();
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            100,
            200,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);

        let new_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            100,
            100 + 365 * 86_400,
            "session.listen",
            None,
        );

        let make_cfg = |hook: Arc<dyn Fn() + Send + Sync>| RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let new_tok = new_tok.clone();
                move |_, _, _, _| Ok(new_tok.clone())
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-2".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(|| 1000), // expired
                now_mono: Arc::new(|| Duration::from_secs(1000)),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: hook,
        };

        // 1. Hook changes token string to different still-expired token
        let hook_tok = "hook-different-expired-token";
        let hook1 = Arc::new({
            let root = journal.path().to_path_buf();
            move || {
                save_service_token(&root, hook_tok).unwrap();
            }
        });
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(hook1), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Expired,
                outcome: Outcome::Superseded
            }
        );
        match crate::link_state_files::load_link_service_token(journal.path()) {
            LinkServiceTokenRead::Present(t) => assert_eq!(t.as_str(), hook_tok),
            _ => panic!("token not present"),
        }
        assert_eq!(state.backoff_deadline, Duration::ZERO);

        // One more evaluation enrolls again (outcome Renewed)
        let step1_retry = evaluate_once(&make_cfg(Arc::new(|| {})), &mut state);
        assert_eq!(
            step1_retry,
            RenewalStep::Attempted {
                trigger: Trigger::Undecodable,
                outcome: Outcome::Renewed,
            }
        );
        assert_eq!(state.backoff_deadline, Duration::ZERO);

        // 2. Hook sets posture to direct
        save_service_token(journal.path(), "expired-token").unwrap();
        let hook2 = Arc::new({
            let root = journal.path().to_path_buf();
            move || {
                write_test_journal_config(&root, "direct", None);
            }
        });
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(hook2), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Undecodable,
                outcome: Outcome::Superseded
            }
        );
        match crate::link_state_files::load_link_service_token(journal.path()) {
            LinkServiceTokenRead::Present(t) => assert_eq!(t.as_str(), "expired-token"),
            _ => panic!("token not present"),
        }
        // Restore posture to spl
        write_test_journal_config(journal.path(), "spl", Some("https://relay.example.com"));

        // 3. Hook changes link.relay_url
        let hook3 = Arc::new({
            let root = journal.path().to_path_buf();
            move || {
                write_test_journal_config(&root, "spl", Some("https://other.example.com"));
            }
        });
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(hook3), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Undecodable,
                outcome: Outcome::Superseded
            }
        );
        // Restore relay URL
        write_test_journal_config(journal.path(), "spl", Some("https://relay.example.com"));

        // 4. No hook change: new token on disk, mode 0o600 on unix, config/identity/CA unchanged
        let cfg_file_before = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        let id_before = std::fs::read(journal.path().join("link/state.json")).unwrap();
        let cert_before = std::fs::read(journal.path().join("link/ca/cert.pem")).unwrap();
        let key_before = std::fs::read(journal.path().join("link/ca/private.pem")).unwrap();

        let hook4 = Arc::new(|| {});
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(hook4), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Undecodable,
                outcome: Outcome::Renewed
            }
        );
        match crate::link_state_files::load_link_service_token(journal.path()) {
            LinkServiceTokenRead::Present(t) => assert_eq!(t.as_str(), new_tok),
            _ => panic!("token not present"),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(journal.path().join("link/tokens/account.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            cfg_file_before
        );
        assert_eq!(
            std::fs::read(journal.path().join("link/state.json")).unwrap(),
            id_before
        );
        assert_eq!(
            std::fs::read(journal.path().join("link/ca/cert.pem")).unwrap(),
            cert_before
        );
        assert_eq!(
            std::fs::read(journal.path().join("link/ca/private.pem")).unwrap(),
            key_before
        );

        // 5. Hook calls enable_spl_with with stub enroll
        save_service_token(journal.path(), "expired-token").unwrap();
        let stub_tok = "stub-token-from-enable-spl-with";
        let hook5 = Arc::new({
            let root = journal.path().to_path_buf();
            move || {
                enable_spl_with(&root, |_, _| Ok(stub_tok.into()), || true).unwrap();
            }
        });
        let mut state = RenewalState::default();
        let step = evaluate_once(&make_cfg(hook5), &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Undecodable,
                outcome: Outcome::Superseded
            }
        );
        match crate::link_state_files::load_link_service_token(journal.path()) {
            LinkServiceTokenRead::Present(t) => assert_eq!(t.as_str(), stub_tok),
            _ => panic!("token not present"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn paused_loop_defers_first_evaluation_until_full_tick() {
        let journal = TempJournal::new();
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            100,
            200,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);

        let new_tok = make_test_jwt(
            Some("kid-test-2"),
            &inst_id,
            100,
            100 + 365 * 86_400,
            "session.listen",
            None,
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Arc::new(Mutex::new(tx));

        let cfg = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let tx = tx.clone();
                let new_tok = new_tok.clone();
                move |_, _, _, _| {
                    let _ = tx.lock().unwrap().send(());
                    Ok(new_tok.clone())
                }
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-2".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(|| 1000), // expired
                now_mono: Arc::new(|| Duration::from_secs(1000)),
            },
            tick: SERVICE_TOKEN_RENEWAL_TICK,
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        let handle = spawn_service_token_renewal(cfg);

        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());

        tokio::time::advance(SERVICE_TOKEN_RENEWAL_TICK - Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(5)))
            .await
            .unwrap()
            .expect("enroll signaled within 5s");

        handle.abort();
    }

    #[test]
    fn not_enrolled_rejection_applies_conflict_ladder_and_preserves_state() {
        let journal = TempJournal::new();
        let iat = 1_000_000i64;
        let exp = iat + 365 * 86_400;
        let dummy = make_test_jwt(
            Some("kid-test-1"),
            "inst-1",
            iat,
            exp,
            "session.listen",
            None,
        );
        let inst_id = setup_test_journal(&journal, &dummy);

        let share = share_bp(&inst_id);
        let offset = ((exp - iat) * share as i64) / 10_000;
        let threshold = iat + offset;

        let tok = make_test_jwt(
            Some("kid-test-1"),
            &inst_id,
            iat,
            exp,
            "session.listen",
            None,
        );
        save_service_token(journal.path(), &tok).unwrap();

        let tok_before = std::fs::read(journal.path().join("link/tokens/account.json")).unwrap();
        let posture_before = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        let id_before = std::fs::read(journal.path().join("link/state.json")).unwrap();
        let cert_before = std::fs::read(journal.path().join("link/ca/cert.pem")).unwrap();
        let key_before = std::fs::read(journal.path().join("link/ca/private.pem")).unwrap();

        let recorded_modes = Arc::new(Mutex::new(Vec::new()));
        let mono_min = Arc::new(AtomicU64::new(0));

        let cfg = RenewalConfig {
            journal_root: journal.path().to_path_buf(),
            enroll: Arc::new({
                let recorded_modes = recorded_modes.clone();
                move |_, _, _, mode| {
                    recorded_modes.lock().unwrap().push(mode);
                    Err(EnrollError::Rejected {
                        status: 404,
                        reason: Some("not_enrolled".into()),
                    })
                }
            }),
            jwks_fetch: Arc::new(|_| JwksSet::Healthy {
                kids: vec!["kid-test-1".into()],
            }),
            health_read: Arc::new(ServiceTokenHealth::default),
            clock: RenewalClock {
                now_unix: Arc::new(move || threshold), // Age trigger
                now_mono: Arc::new({
                    let mono_min = mono_min.clone();
                    move || Duration::from_secs(mono_min.load(Ordering::SeqCst) * 60)
                }),
            },
            tick: Duration::from_secs(60),
            jitter: Arc::new(|d| d),
            before_commit: Arc::new(|| {}),
        };

        let mut state = RenewalState::default();
        let step = evaluate_once(&cfg, &mut state);
        assert_eq!(
            step,
            RenewalStep::Attempted {
                trigger: Trigger::Age,
                outcome: Outcome::NotEnrolled
            }
        );

        // Token, posture, identity, and CA byte-for-byte unchanged
        assert_eq!(
            std::fs::read(journal.path().join("link/tokens/account.json")).unwrap(),
            tok_before
        );
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            posture_before
        );
        assert_eq!(
            std::fs::read(journal.path().join("link/state.json")).unwrap(),
            id_before
        );
        assert_eq!(
            std::fs::read(journal.path().join("link/ca/cert.pem")).unwrap(),
            cert_before
        );
        assert_eq!(
            std::fs::read(journal.path().join("link/ca/private.pem")).unwrap(),
            key_before
        );

        // Zero jitter schedule: exactly 4 attempts at 0, 360, 720, 1080
        let mut attempts = vec![0];
        for minute in 1..=1439 {
            mono_min.store(minute, Ordering::SeqCst);
            let s = evaluate_once(&cfg, &mut state);
            if matches!(s, RenewalStep::Attempted { .. }) {
                attempts.push(minute);
            }
        }
        assert_eq!(attempts, vec![0, 360, 720, 1080]);

        // Every seam call is EnrollMode::Renew
        let modes = recorded_modes.lock().unwrap().clone();
        assert_eq!(modes.len(), 4);
        assert!(modes.iter().all(|&m| m == EnrollMode::Renew));

        // 404 with reason other or None follows 12-attempt ladder
        let test_404_other = |reason: Option<&str>| {
            let mono_min = Arc::new(AtomicU64::new(0));
            let cfg_other = RenewalConfig {
                enroll: Arc::new({
                    let r = reason.map(str::to_owned);
                    move |_, _, _, _| {
                        Err(EnrollError::Rejected {
                            status: 404,
                            reason: r.clone(),
                        })
                    }
                }),
                clock: RenewalClock {
                    now_unix: Arc::new(move || threshold),
                    now_mono: Arc::new({
                        let mono_min = mono_min.clone();
                        move || Duration::from_secs(mono_min.load(Ordering::SeqCst) * 60)
                    }),
                },
                ..cfg.clone()
            };
            let mut s = RenewalState::default();
            let mut attempts = Vec::new();
            for minute in 0..=1439 {
                mono_min.store(minute, Ordering::SeqCst);
                let step = evaluate_once(&cfg_other, &mut s);
                if matches!(
                    step,
                    RenewalStep::Attempted {
                        outcome: Outcome::Rejected { status: 404 },
                        ..
                    }
                ) {
                    attempts.push(minute);
                }
            }
            assert_eq!(
                attempts,
                vec![0, 1, 3, 7, 15, 31, 63, 127, 255, 511, 871, 1231]
            );
        };
        test_404_other(Some("other"));
        test_404_other(None);
    }
}
