// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Brain's process-lifetime credential refusal state. Durable authority remains
//! in config and the Brain record; this owner keeps an unacknowledged rejection
//! across individual Generate calls until a fresh conditional validation or death.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use solstone_core_journal_io::{JournalRoot, ObjectIdentity};

use crate::{endpoint_credential_binding_hmac, managed_credential_binding_hmac};

// Empty roots are reclaimable. Held roots have no expiry or eviction policy.
// Exhaustion refuses affected credential work before sending content.
const MAX_LIVE_ROOTS: usize = 32;
// Count both held targets and still-running prework witnesses. Never evict a
// refusal or a cell from which an old operation can still return.
const MAX_LIVE_TARGETS_PER_PROVIDER: usize = 8;

/// Actual sent credential identity. This type deliberately has no Debug or
/// serialization implementation; only salted HMACs enter the process cache.
#[derive(Clone, Copy)]
pub enum CredentialHoldTarget<'a> {
    Managed {
        provider: &'a str,
        credential: &'a str,
    },
    Endpoint {
        base_url: &'a str,
        credential: &'a str,
    },
}

impl CredentialHoldTarget<'_> {
    pub(crate) fn provider_name(self) -> Result<&'static str, CredentialHoldError> {
        match self {
            Self::Managed { provider, .. } => match Provider::parse(provider)? {
                Provider::Local => Err(CredentialHoldError::InvalidProvider),
                provider => Ok(provider.name()),
            },
            Self::Endpoint { .. } => Ok("local"),
        }
    }

    pub(crate) fn binding_hmac(self, key: &[u8; 32]) -> Result<String, CredentialHoldError> {
        let provider = self.provider_name()?;
        Ok(match self {
            Self::Managed { credential, .. } => {
                managed_credential_binding_hmac(key, provider, credential)
            }
            Self::Endpoint {
                base_url,
                credential,
            } => endpoint_credential_binding_hmac(key, base_url, credential),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialHoldError {
    InvalidProvider,
    Capacity,
    Unavailable,
    RootChanged,
    Superseded,
}

impl CredentialHoldError {
    pub(crate) fn message_id(&self) -> &'static str {
        match self {
            Self::InvalidProvider => "journal.cli.brain.hold.invalid.provider",
            Self::Capacity => "journal.cli.brain.hold.capacity",
            Self::Unavailable => "journal.cli.brain.hold.unavailable",
            Self::RootChanged => "journal.cli.brain.hold.root.changed",
            Self::Superseded => "journal.cli.brain.hold.superseded",
        }
    }
}

impl fmt::Display for CredentialHoldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::ENGLISH,
            self.message_id(),
            &[],
        ))
    }
}

impl std::error::Error for CredentialHoldError {}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Provider {
    Openai,
    Anthropic,
    Google,
    Local,
}

impl Provider {
    fn parse(provider: &str) -> Result<Self, CredentialHoldError> {
        match provider {
            "openai" => Ok(Self::Openai),
            "anthropic" => Ok(Self::Anthropic),
            "google" => Ok(Self::Google),
            "local" => Ok(Self::Local),
            _ => Err(CredentialHoldError::InvalidProvider),
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
            Self::Local => "local",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Hold {
    target_hmac: String,
    marker: String,
    origin: Option<CredentialHoldOrigin>,
}

/// Bounded provenance from the existing durable carriers at admission. The
/// exceptional positive is an older same-process publication already in flight
/// when this negative was learned; it cannot certify correction of this hold.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CredentialHoldOrigin {
    pub config_id: Option<String>,
    pub brain_id: Option<String>,
    pub binding_material_id: Option<String>,
    pub older_publications: Vec<PositivePublication>,
    pub excluded_publication: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PositivePublication {
    serial: u64,
    target_hmac: String,
    pub observation_id: String,
    pub removed_brain_hold: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationCarrier {
    Config,
    Brain,
}

#[derive(Clone, PartialEq, Eq)]
struct PositiveReservation {
    publication: PositivePublication,
    carrier: ObservationCarrier,
    active: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct ProvisionalHold {
    hold: Hold,
    serial: u64,
    generation: u64,
}

#[derive(Default)]
struct RootHolds {
    generations: [u64; 4],
    holds: HashMap<Provider, Hold>,
    provisionals: HashMap<Provider, ProvisionalHold>,
    publications: HashMap<Provider, PositiveReservation>,
    attempts: HashMap<Provider, [Option<PositivePublication>; 2]>,
}

struct RootState {
    root: Arc<JournalRoot>,
    targets: Mutex<TargetEntries>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct TargetKey {
    provider: Provider,
    hmac: String,
}

#[derive(Default)]
struct TargetEntries {
    cells: HashMap<TargetKey, Weak<TargetState>>,
    pinned: HashMap<TargetKey, Arc<TargetState>>,
}

struct TargetState {
    root: Arc<JournalRoot>,
    owner: Arc<RootState>,
    key: TargetKey,
    holds: Mutex<RootHolds>,
}

impl RootState {
    fn admit_target(
        self: &Arc<Self>,
        key: TargetKey,
    ) -> Result<Arc<TargetState>, CredentialHoldError> {
        let mut entries = self
            .targets
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        entries.cells.retain(|_, cell| cell.strong_count() != 0);
        if let Some(cell) = entries.cells.get(&key).and_then(Weak::upgrade) {
            return Ok(cell);
        }
        if entries
            .cells
            .keys()
            .filter(|other| other.provider == key.provider)
            .count()
            >= MAX_LIVE_TARGETS_PER_PROVIDER
        {
            return Err(CredentialHoldError::Capacity);
        }
        let state = Arc::new(TargetState {
            root: self.root.clone(),
            owner: self.clone(),
            key: key.clone(),
            holds: Mutex::default(),
        });
        entries.cells.insert(key, Arc::downgrade(&state));
        Ok(state)
    }
}

#[derive(Default)]
struct RegistryEntries {
    roots: HashMap<ObjectIdentity, Weak<RootState>>,
    pinned: HashMap<ObjectIdentity, Arc<RootState>>,
}

struct HoldRegistry {
    salt: [u8; 32],
    max_roots: usize,
    entries: Mutex<RegistryEntries>,
    sequence: AtomicU64,
}

impl HoldRegistry {
    fn retire_unselectable(&self, owner: &Arc<RootState>) -> Result<(), CredentialHoldError> {
        use solstone_core_journal_config::read_journal_config_bound;
        use solstone_core_journal_io::LockOptions;
        let initial =
            read_journal_config_bound(&owner.root).map_err(|_| CredentialHoldError::Capacity)?;
        let expected = initial.sha256.ok_or(CredentialHoldError::Capacity)?;
        let retired = std::cell::RefCell::new(None);
        let outcome =
            solstone_core_journal_config_write::conditional_update_journal_config_at_root_checked(
                &owner.root,
                &expected,
                |config| {
                    // The cooperating config owner holds its original named C
                    // guard here. This callback always declines a durable update.
                    let override_key = current_process_key_override();
                    let protected = match self.selectable_targets(config, override_key.as_deref()) {
                        Ok(protected) => protected,
                        Err(error) => {
                            *retired.borrow_mut() = Some(Err(error));
                            return None;
                        }
                    };
                    // The maintained override is immutable launch environment.
                    // Equal reads alone are not a mutable-selector lock.
                    if current_process_key_override() != override_key {
                        *retired.borrow_mut() = Some(Err(CredentialHoldError::Capacity));
                        return None;
                    }
                    *retired.borrow_mut() = Some(self.retire_quiescent_cells(owner, &protected));
                    None
                },
                LockOptions {
                    timeout: std::time::Duration::ZERO,
                    ..LockOptions::default()
                },
                &|| {
                    owner
                        .root
                        .revalidate_canonical_binding()
                        .map_err(|error| error.to_string())
                },
            )
            .map_err(|_| CredentialHoldError::Capacity)?;
        if !matches!(
            outcome,
            solstone_core_journal_config_write::BoundConfigOutcome::Unchanged
        ) {
            return Err(CredentialHoldError::Capacity);
        }
        retired.into_inner().ok_or(CredentialHoldError::Capacity)?
    }

    fn selectable_targets(
        &self,
        config: &serde_json::Map<String, serde_json::Value>,
        override_key: Option<&str>,
    ) -> Result<Vec<TargetKey>, CredentialHoldError> {
        // Malformed selector state is uncertainty, never proof of absence.
        if config.get("env").is_some_and(|value| !value.is_object())
            || config
                .get("providers")
                .is_some_and(|value| !value.is_object())
        {
            return Err(CredentialHoldError::Capacity);
        }
        for name in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GOOGLE_API_KEY"] {
            if config
                .get("env")
                .and_then(|env| env.get(name))
                .is_some_and(|value| !value.is_string() && !value.is_null())
            {
                return Err(CredentialHoldError::Capacity);
            }
        }
        let providers = config.get("providers");
        for name in ["active", "local"] {
            if providers
                .and_then(|providers| providers.get(name))
                .is_some_and(|value| !value.is_object())
            {
                return Err(CredentialHoldError::Capacity);
            }
        }
        for name in ["endpoint_url", "served_model_id", "credential"] {
            if providers
                .and_then(|providers| providers.get("local"))
                .and_then(|local| local.get(name))
                .is_some_and(|value| !value.is_string() && !value.is_null())
            {
                return Err(CredentialHoldError::Capacity);
            }
        }
        let mut protected = Vec::new();
        for (provider, env_key) in [
            (Provider::Openai, "OPENAI_API_KEY"),
            (Provider::Anthropic, "ANTHROPIC_API_KEY"),
            (Provider::Google, "GOOGLE_API_KEY"),
        ] {
            let configured = config
                .get("env")
                .and_then(|env| env.get(env_key))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty());
            for credential in configured.into_iter().chain(override_key) {
                protected.push(TargetKey {
                    provider,
                    hmac: managed_credential_binding_hmac(&self.salt, provider.name(), credential),
                });
            }
        }
        let (base_url, credential) = crate::fingerprint::endpoint_binding_parts(config);
        protected.push(TargetKey {
            provider: Provider::Local,
            hmac: endpoint_credential_binding_hmac(&self.salt, &base_url, &credential),
        });
        Ok(protected)
    }

    fn retire_quiescent_cells(
        &self,
        owner: &Arc<RootState>,
        protected: &[TargetKey],
    ) -> Result<(), CredentialHoldError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        let mut targets = owner
            .targets
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        let candidates = targets
            .pinned
            .keys()
            .filter(|key| !protected.contains(key))
            .filter(|key| {
                targets
                    .cells
                    .get(*key)
                    .is_some_and(|cell| cell.strong_count() == 1)
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in candidates {
            // The table pin is the sole Arc. Every prework witness, receipt,
            // permit and publication guard owns an Arc, including late returns.
            // Target-table locking prevents a new admission during removal.
            let cell = targets.pinned.get(&key).expect("candidate remains pinned");
            let state = cell
                .holds
                .lock()
                .map_err(|_| CredentialHoldError::Unavailable)?;
            if state
                .publications
                .values()
                .any(|publication| publication.active)
            {
                continue;
            }
            drop(state);
            targets.pinned.remove(&key);
            targets.cells.remove(&key);
        }
        if targets.pinned.is_empty() {
            entries.pinned.remove(&owner.root.identity());
        }
        Ok(())
    }

    fn new(salt: [u8; 32], max_roots: usize) -> Self {
        Self {
            salt,
            max_roots,
            entries: Mutex::default(),
            sequence: AtomicU64::new(0),
        }
    }

    fn admit(&self, root: Arc<JournalRoot>) -> Result<Arc<RootState>, CredentialHoldError> {
        root.revalidate_canonical_binding()
            .map_err(|_| CredentialHoldError::RootChanged)?;
        let identity = root.identity();
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        entries.roots.retain(|_, root| root.strong_count() != 0);
        if let Some(current) = entries.roots.get(&identity).and_then(Weak::upgrade) {
            return Ok(current);
        }
        if entries.roots.len() >= self.max_roots {
            return Err(CredentialHoldError::Capacity);
        }
        let state = Arc::new(RootState {
            root,
            targets: Mutex::default(),
        });
        entries.roots.insert(identity, Arc::downgrade(&state));
        Ok(state)
    }

    fn sync_pin(&self, state: &Arc<TargetState>) -> Result<(), CredentialHoldError> {
        // Registry -> root target table -> target state is the only nested
        // order. No callback, probe or filesystem work runs under these mutexes.
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        let mut targets = state
            .owner
            .targets
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        let holds = state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        if holds.holds.is_empty() && holds.provisionals.is_empty() {
            targets.pinned.remove(&state.key);
        } else {
            targets.pinned.insert(state.key.clone(), Arc::clone(state));
        }
        if targets.pinned.is_empty() {
            entries.pinned.remove(&state.root.identity());
        } else {
            entries
                .pinned
                .insert(state.root.identity(), state.owner.clone());
        }
        Ok(())
    }
}

static PROCESS_HOLDS: OnceLock<Result<Arc<HoldRegistry>, CredentialHoldError>> = OnceLock::new();

fn current_process_key_override() -> Option<String> {
    std::env::var(solstone_core_journal_config::GENERATE_API_KEY_OVERRIDE_ENV)
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

fn process_holds() -> Result<Arc<HoldRegistry>, CredentialHoldError> {
    PROCESS_HOLDS
        .get_or_init(|| {
            let mut salt = [0; 32];
            getrandom::fill(&mut salt).map_err(|_| CredentialHoldError::Unavailable)?;
            Ok(Arc::new(HoldRegistry::new(salt, MAX_LIVE_ROOTS)))
        })
        .clone()
}

/// A per-operation handle into Brain's shared process state. Dropping this
/// handle does not discard a rejection; the registry strongly pins held roots.
#[derive(Clone)]
pub struct CredentialHoldSession {
    registry: Arc<HoldRegistry>,
    state: Arc<TargetState>,
    provider: Provider,
    target_hmac: String,
}

/// Snapshot taken before the actual mutating owner's authoritative probe.
/// Durable Brain/config witnesses are composed with this generation by their
/// existing owners. This volatile witness alone grants no publication authority.
pub struct VolatileCredentialWitness {
    session: CredentialHoldSession,
    generation: u64,
    matched_marker: Option<String>,
    matched_provisional: Option<String>,
    provisional_marker_seen: Option<String>,
    serial: u64,
}

/// A negative blocks content before sidecar waits, without claiming an accepted
/// generation. Dropping this token never discards the learned rejection.
pub(crate) struct ProvisionalCredentialRejection {
    session: CredentialHoldSession,
    provisional: ProvisionalHold,
}

/// Bounded reservation for one provider's positive native publication. Its ID
/// is minted by the actual validation owner, never by the registry.
pub(crate) struct CredentialPositivePublication {
    session: CredentialHoldSession,
    publication: PositivePublication,
    carrier: ObservationCarrier,
}

impl Drop for CredentialPositivePublication {
    fn drop(&mut self) {
        if let Ok(mut state) = self.session.state.holds.lock()
            && let Some(current) = state.publications.get_mut(&self.session.provider)
            && current.publication == self.publication
            && current.carrier == self.carrier
        {
            // A later strict carrier snapshot resolves whether native IO
            // actually replaced the previous slot. Drop is never settlement.
            current.active = false;
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CredentialMemorySnapshot {
    generation: u64,
    accepted: Option<Hold>,
    provisional: Option<ProvisionalHold>,
    publication: Option<PositiveReservation>,
    attempts: [Option<PositivePublication>; 2],
}

impl CredentialMemorySnapshot {
    pub(crate) fn has_matching_rejection(&self, session: &CredentialHoldSession) -> bool {
        !self.matching_origins(session).is_empty()
    }

    pub(crate) fn corrected_by(
        &self,
        session: &CredentialHoldSession,
        key: &[u8; 32],
        positives: &[String],
    ) -> bool {
        let origins = self.matching_origins(session);
        !origins.is_empty()
            && origins.iter().all(|origin| {
                origin.is_some_and(|origin| {
                    origin
                        .binding_material_id
                        .as_ref()
                        .is_none_or(|original| original == &session.binding_material_id(key))
                        && positives.iter().any(|id| {
                            Some(id) != origin.config_id.as_ref()
                                && Some(id) != origin.brain_id.as_ref()
                                && origin
                                    .older_publications
                                    .iter()
                                    .all(|p| &p.observation_id != id)
                                && origin.excluded_publication.as_ref() != Some(id)
                                && self
                                    .publication
                                    .as_ref()
                                    .is_none_or(|p| &p.publication.observation_id != id)
                                && self
                                    .attempts
                                    .iter()
                                    .flatten()
                                    .all(|p| &p.observation_id != id)
                        })
                })
            })
    }

    fn matching_origins(
        &self,
        session: &CredentialHoldSession,
    ) -> Vec<Option<&CredentialHoldOrigin>> {
        self.accepted
            .iter()
            .chain(self.provisional.iter().map(|p| &p.hold))
            .filter(|hold| hold.target_hmac == session.target_hmac)
            .map(|hold| hold.origin.as_ref())
            .collect()
    }
}

/// Provider-generation fence for refresh publication. It grants no credential
/// identity or hold-retirement authority.
pub(crate) struct CredentialProviderGeneration {
    state: Arc<TargetState>,
    provider: Provider,
    generation: u64,
    provisional_marker: Option<String>,
}

impl fmt::Debug for CredentialProviderGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialProviderGeneration")
            .field("provider", &self.provider.name())
            .field("generation", &self.generation)
            .finish()
    }
}

impl CredentialProviderGeneration {
    pub(crate) fn same_target(&self, witness: &VolatileCredentialWitness) -> bool {
        Arc::ptr_eq(&self.state, &witness.session.state)
            && self.provider == witness.session.provider
    }
    pub(crate) fn capture(
        root: Arc<JournalRoot>,
        target: CredentialHoldTarget<'_>,
    ) -> Result<Self, CredentialHoldError> {
        let session = CredentialHoldSession::open_bound(root, target)?;
        let provider = session.provider;
        let state = session.state;
        let (generation, provisional_marker) = {
            let holds = state
                .holds
                .lock()
                .map_err(|_| CredentialHoldError::Unavailable)?;
            (
                holds.generations[provider.index()],
                holds
                    .provisionals
                    .get(&provider)
                    .map(|p| p.hold.marker.clone()),
            )
        };
        Ok(Self {
            state,
            provider,
            generation,
            provisional_marker,
        })
    }

    pub(crate) fn ensure_current(&self) -> Result<(), CredentialHoldError> {
        self.state
            .root
            .revalidate_canonical_binding()
            .map_err(|_| CredentialHoldError::RootChanged)?;
        let state = self
            .state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        if state.generations[self.provider.index()] != self.generation
            || state
                .provisionals
                .get(&self.provider)
                .map(|p| &p.hold.marker)
                != self.provisional_marker.as_ref()
        {
            return Err(CredentialHoldError::Superseded);
        }
        Ok(())
    }
}

impl CredentialHoldSession {
    pub fn open(
        journal: &Path,
        target: CredentialHoldTarget<'_>,
    ) -> Result<Self, CredentialHoldError> {
        let root =
            Arc::new(JournalRoot::open(journal).map_err(|_| CredentialHoldError::RootChanged)?);
        Self::open_bound(root, target)
    }

    /// Use the same admitted root capability as the operation's durable owner.
    pub fn open_bound(
        root: Arc<JournalRoot>,
        target: CredentialHoldTarget<'_>,
    ) -> Result<Self, CredentialHoldError> {
        Self::admit(process_holds()?, root, target)
    }

    /// Inspect only an existing target cell. Reading owner state cannot
    /// initialize the registry, allocate a cell or retire another target.
    pub(crate) fn existing_bound(
        root: Arc<JournalRoot>,
        target: CredentialHoldTarget<'_>,
    ) -> Result<Option<Self>, CredentialHoldError> {
        root.revalidate_canonical_binding()
            .map_err(|_| CredentialHoldError::RootChanged)?;
        let Some(registry) = PROCESS_HOLDS.get() else {
            return Ok(None);
        };
        let registry = registry.clone()?;
        let provider = Provider::parse(target.provider_name()?)?;
        let target_hmac = target.binding_hmac(&registry.salt)?;
        let owner = registry
            .entries
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?
            .roots
            .get(&root.identity())
            .and_then(Weak::upgrade);
        let Some(owner) = owner else {
            return Ok(None);
        };
        let key = TargetKey {
            provider,
            hmac: target_hmac.clone(),
        };
        let state = owner
            .targets
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?
            .cells
            .get(&key)
            .and_then(Weak::upgrade);
        let Some(state) = state else {
            return Ok(None);
        };
        let session = Self {
            registry,
            state,
            provider,
            target_hmac,
        };
        session.checkpoint()?;
        Ok(Some(session))
    }

    fn admit(
        registry: Arc<HoldRegistry>,
        root: Arc<JournalRoot>,
        target: CredentialHoldTarget<'_>,
    ) -> Result<Self, CredentialHoldError> {
        let (provider, target_hmac) = match target {
            CredentialHoldTarget::Managed {
                provider,
                credential,
            } => {
                let provider = Provider::parse(provider)?;
                if provider == Provider::Local {
                    return Err(CredentialHoldError::InvalidProvider);
                }
                (
                    provider,
                    managed_credential_binding_hmac(&registry.salt, provider.name(), credential),
                )
            }
            CredentialHoldTarget::Endpoint {
                base_url,
                credential,
            } => (
                Provider::Local,
                endpoint_credential_binding_hmac(&registry.salt, base_url, credential),
            ),
        };
        let owner = registry.admit(root)?;
        let key = TargetKey {
            provider,
            hmac: target_hmac.clone(),
        };
        let state = match owner.admit_target(key.clone()) {
            Err(CredentialHoldError::Capacity) => {
                registry.retire_unselectable(&owner)?;
                owner.admit_target(key)?
            }
            result => result?,
        };
        Ok(Self {
            registry,
            state,
            provider,
            target_hmac,
        })
    }

    fn checkpoint(&self) -> Result<(), CredentialHoldError> {
        self.state
            .root
            .revalidate_canonical_binding()
            .map_err(|_| CredentialHoldError::RootChanged)
    }

    pub fn is_held(&self) -> Result<bool, CredentialHoldError> {
        self.checkpoint()?;
        let state = self
            .state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        Ok(state
            .holds
            .get(&self.provider)
            .is_some_and(|hold| hold.target_hmac == self.target_hmac)
            || state
                .provisionals
                .get(&self.provider)
                .is_some_and(|p| p.hold.target_hmac == self.target_hmac))
    }

    /// Register only after the caller establishes a definitive rejection and
    /// original-root, cancellation and actual-current-target authority. A target
    /// supersedes this provider's old slot only under that current-target proof.
    /// Register before the potentially blocking config/Brain durability attempts.
    pub fn record_rejection(&self) -> Result<String, CredentialHoldError> {
        let before = self.validation_witness()?;
        self.record_rejection_after(&before)
            .map(|(marker, _)| marker)
    }

    /// Renew current-target authority after taking `before`, then conditionally
    /// register against that exact generation. The returned marker and writer
    /// witness belong to the same atomic registration, never a later snapshot.
    pub fn record_rejection_after(
        &self,
        before: &VolatileCredentialWitness,
    ) -> Result<(String, VolatileCredentialWitness), CredentialHoldError> {
        let provisional = self.prepare_rejection(before, None)?;
        let marker = provisional.marker().to_owned();
        Ok((marker, provisional.promote()?))
    }

    pub(crate) fn prepare_rejection(
        &self,
        before: &VolatileCredentialWitness,
        mut origin: Option<CredentialHoldOrigin>,
    ) -> Result<ProvisionalCredentialRejection, CredentialHoldError> {
        self.checkpoint()?;
        if !Arc::ptr_eq(&self.state, &before.session.state)
            || self.provider != before.session.provider
            || self.target_hmac != before.session.target_hmac
        {
            return Err(CredentialHoldError::Superseded);
        }
        let sequence = self
            .registry
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| CredentialHoldError::Unavailable)?;
        let marker = managed_credential_binding_hmac(
            &self.registry.salt,
            "credential-hold-marker",
            &format!(
                "{}\0{}\0{sequence}",
                self.state.root.identity().observation_id(),
                self.provider.name()
            ),
        );
        let provisional = {
            let mut state = self
                .state
                .holds
                .lock()
                .map_err(|_| CredentialHoldError::Unavailable)?;
            if state.generations[self.provider.index()] != before.generation
                || state
                    .provisionals
                    .get(&self.provider)
                    .is_some_and(|p| p.serial >= before.serial)
            {
                return Err(CredentialHoldError::Superseded);
            }
            // Preparing the preblocking fact cannot depend on a fallible new
            // carrier read or discard unresolved native publication provenance.
            if let Some(origin) = &mut origin {
                origin.older_publications = state
                    .attempts
                    .get(&self.provider)
                    .into_iter()
                    .flat_map(|pair| pair.iter().flatten())
                    .chain(
                        state
                            .publications
                            .get(&self.provider)
                            .map(|p| &p.publication),
                    )
                    .filter(|p| p.serial < before.serial && p.target_hmac == self.target_hmac)
                    .take(3)
                    .cloned()
                    .collect();
            }
            let provisional = ProvisionalHold {
                hold: Hold {
                    target_hmac: self.target_hmac.clone(),
                    marker,
                    origin,
                },
                serial: before.serial,
                generation: before.generation,
            };
            state
                .provisionals
                .insert(self.provider, provisional.clone());
            provisional
        };
        self.registry.sync_pin(&self.state)?;
        Ok(ProvisionalCredentialRejection {
            session: self.clone(),
            provisional,
        })
    }

    pub(crate) fn binding_material_id(&self, key: &[u8; 32]) -> String {
        let material: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
        managed_credential_binding_hmac(
            &self.registry.salt,
            "protected-binding-material",
            &material,
        )
    }

    pub(crate) fn memory_snapshot(&self) -> Result<CredentialMemorySnapshot, CredentialHoldError> {
        self.checkpoint()?;
        let state = self
            .state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        Ok(CredentialMemorySnapshot {
            generation: state.generations[self.provider.index()],
            accepted: state.holds.get(&self.provider).cloned(),
            provisional: state.provisionals.get(&self.provider).cloned(),
            publication: state.publications.get(&self.provider).cloned(),
            attempts: state
                .attempts
                .get(&self.provider)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// The caller reads strict existing durable owners outside this mutex and
    /// proves every matching refusal obsolete. A new provisional invalidates
    /// this CAS even though preparing it did not advance the accepted epoch.
    pub(crate) fn reconcile(
        &self,
        before: &CredentialMemorySnapshot,
    ) -> Result<bool, CredentialHoldError> {
        self.checkpoint()?;
        let removed = {
            let mut state = self
                .state
                .holds
                .lock()
                .map_err(|_| CredentialHoldError::Unavailable)?;
            if state.generations[self.provider.index()] != before.generation
                || state.holds.get(&self.provider) != before.accepted.as_ref()
                || state.provisionals.get(&self.provider) != before.provisional.as_ref()
                || state.publications.get(&self.provider) != before.publication.as_ref()
                || state
                    .attempts
                    .get(&self.provider)
                    .cloned()
                    .unwrap_or_default()
                    != before.attempts
            {
                return Err(CredentialHoldError::Superseded);
            }
            let accepted = before
                .accepted
                .as_ref()
                .is_some_and(|h| h.target_hmac == self.target_hmac);
            let provisional = before
                .provisional
                .as_ref()
                .is_some_and(|p| p.hold.target_hmac == self.target_hmac);
            if !accepted && !provisional {
                return Ok(false);
            }
            if before
                .provisional
                .as_ref()
                .is_some_and(|p| p.hold.target_hmac != self.target_hmac)
            {
                return Err(CredentialHoldError::Superseded);
            }
            let next = before
                .generation
                .checked_add(1)
                .ok_or(CredentialHoldError::Unavailable)?;
            if accepted {
                state.holds.remove(&self.provider);
            }
            if provisional {
                state.provisionals.remove(&self.provider);
            }
            state.generations[self.provider.index()] = next;
            true
        };
        self.registry.sync_pin(&self.state)?;
        Ok(removed)
    }

    /// Reconcile/capture using the SAME caller-owned durable snapshot. The
    /// complete provider memory CAS prevents a new P/reservation or provenance
    /// update from being hidden by an unchanged accepted epoch.
    pub(crate) fn validation_witness_after_snapshot(
        &self,
        before: &CredentialMemorySnapshot,
        reconcile: bool,
        current_ids: &[Option<String>; 2],
    ) -> Result<VolatileCredentialWitness, CredentialHoldError> {
        self.checkpoint()?;
        let witness = {
            let mut state = self
                .state
                .holds
                .lock()
                .map_err(|_| CredentialHoldError::Unavailable)?;
            if state.generations[self.provider.index()] != before.generation
                || state.holds.get(&self.provider) != before.accepted.as_ref()
                || state.provisionals.get(&self.provider) != before.provisional.as_ref()
                || state.publications.get(&self.provider) != before.publication.as_ref()
                || state
                    .attempts
                    .get(&self.provider)
                    .cloned()
                    .unwrap_or_default()
                    != before.attempts
            {
                return Err(CredentialHoldError::Superseded);
            }
            let serial = self
                .registry
                .sequence
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    current.checked_add(1)
                })
                .map_err(|_| CredentialHoldError::Unavailable)?;
            let accepted = before
                .accepted
                .as_ref()
                .is_some_and(|h| h.target_hmac == self.target_hmac);
            let provisional = before
                .provisional
                .as_ref()
                .is_some_and(|p| p.hold.target_hmac == self.target_hmac);
            if reconcile && (accepted || provisional) {
                if before
                    .provisional
                    .as_ref()
                    .is_some_and(|p| p.hold.target_hmac != self.target_hmac)
                {
                    return Err(CredentialHoldError::Superseded);
                }
                let next = before
                    .generation
                    .checked_add(1)
                    .ok_or(CredentialHoldError::Unavailable)?;
                if accepted {
                    state.holds.remove(&self.provider);
                }
                if provisional {
                    state.provisionals.remove(&self.provider);
                }
                state.generations[self.provider.index()] = next;
            }
            resolve_attempts(&mut state, self.provider, current_ids);
            VolatileCredentialWitness {
                session: self.clone(),
                serial,
                generation: state.generations[self.provider.index()],
                matched_marker: state
                    .holds
                    .get(&self.provider)
                    .filter(|h| h.target_hmac == self.target_hmac)
                    .map(|h| h.marker.clone()),
                matched_provisional: state
                    .provisionals
                    .get(&self.provider)
                    .filter(|p| p.hold.target_hmac == self.target_hmac)
                    .map(|p| p.hold.marker.clone()),
                provisional_marker_seen: state
                    .provisionals
                    .get(&self.provider)
                    .map(|p| p.hold.marker.clone()),
            }
        };
        self.registry.sync_pin(&self.state)?;
        Ok(witness)
    }

    pub fn validation_witness(&self) -> Result<VolatileCredentialWitness, CredentialHoldError> {
        self.checkpoint()?;
        let state = self
            .state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        let serial = self
            .registry
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| CredentialHoldError::Unavailable)?;
        Ok(VolatileCredentialWitness {
            session: self.clone(),
            serial,
            matched_provisional: state
                .provisionals
                .get(&self.provider)
                .filter(|p| p.hold.target_hmac == self.target_hmac)
                .map(|p| p.hold.marker.clone()),
            provisional_marker_seen: state
                .provisionals
                .get(&self.provider)
                .map(|p| p.hold.marker.clone()),
            generation: state.generations[self.provider.index()],
            matched_marker: state
                .holds
                .get(&self.provider)
                .filter(|hold| hold.target_hmac == self.target_hmac)
                .map(|hold| hold.marker.clone()),
        })
    }
}

impl VolatileCredentialWitness {
    pub(crate) fn same_target(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.session.state, &other.session.state)
            && self.session.provider == other.session.provider
    }
    pub(crate) fn session_handle(&self) -> CredentialHoldSession {
        self.session.clone()
    }
    pub(crate) fn observed_matching_hold(&self) -> bool {
        self.matched_marker.is_some() || self.matched_provisional.is_some()
    }

    pub(crate) fn provider_generation(&self) -> CredentialProviderGeneration {
        CredentialProviderGeneration {
            state: self.session.state.clone(),
            provider: self.session.provider,
            generation: self.generation,
            // Exact pre-work (or promotion) snapshot, never a later adoption.
            provisional_marker: self.provisional_marker_seen.clone(),
        }
    }

    pub fn ensure_current(&self) -> Result<(), CredentialHoldError> {
        self.session.checkpoint()?;
        let state = self
            .session
            .state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        if state.generations[self.session.provider.index()] != self.generation
            || state
                .provisionals
                .get(&self.session.provider)
                .is_some_and(|p| p.serial >= self.serial)
        {
            return Err(CredentialHoldError::Superseded);
        }
        Ok(())
    }

    pub(crate) fn reserve_positive(
        &self,
        observation_id: &str,
        removed_brain_hold: Option<String>,
        carrier: ObservationCarrier,
        current_ids: &[Option<String>; 2],
        material_id: &str,
    ) -> Result<CredentialPositivePublication, CredentialHoldError> {
        self.ensure_current()?;
        let publication = PositivePublication {
            serial: self.serial,
            target_hmac: self.session.target_hmac.clone(),
            observation_id: observation_id.to_owned(),
            removed_brain_hold,
        };
        let mut state = self
            .session
            .state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        if state.generations[self.session.provider.index()] != self.generation
            || state
                .provisionals
                .get(&self.session.provider)
                .is_some_and(|p| p.serial >= self.serial)
        {
            return Err(CredentialHoldError::Superseded);
        }
        resolve_attempts(&mut state, self.session.provider, current_ids);
        if state.publications.contains_key(&self.session.provider) {
            return Err(CredentialHoldError::Superseded);
        }
        // A late older P may predate a genuine external correction that this
        // positive already observed. Do not absorb that E into P's origin.
        // Only the old pair and specifically known local partial work may be
        // bookkeeping baselines; arbitrary current IDs preserve/refuse P.
        let known: Vec<_> = state
            .attempts
            .get(&self.session.provider)
            .into_iter()
            .flat_map(|pair| pair.iter().flatten())
            .filter(|p| p.target_hmac == self.session.target_hmac)
            .map(|p| p.observation_id.clone())
            .collect();
        for hold in state
            .holds
            .get(&self.session.provider)
            .into_iter()
            .chain(
                state
                    .provisionals
                    .get(&self.session.provider)
                    .map(|p| &p.hold),
            )
            .filter(|h| h.target_hmac == self.session.target_hmac)
        {
            if let Some(origin) = &hold.origin {
                for id in current_ids.iter().flatten() {
                    if Some(id) != origin.config_id.as_ref()
                        && Some(id) != origin.brain_id.as_ref()
                        && !known.contains(id)
                        && origin
                            .older_publications
                            .iter()
                            .all(|p| &p.observation_id != id)
                        && origin.excluded_publication.as_ref() != Some(id)
                    {
                        return Err(CredentialHoldError::Superseded);
                    }
                }
            }
        }
        let rebase = |hold: &mut Hold| {
            if hold.target_hmac != self.session.target_hmac {
                return;
            }
            let origin = hold.origin.get_or_insert_with(|| CredentialHoldOrigin {
                config_id: None,
                brain_id: None,
                binding_material_id: Some(material_id.to_owned()),
                older_publications: Vec::new(),
                excluded_publication: None,
            });
            origin.config_id = current_ids[0].clone();
            origin.brain_id = current_ids[1].clone();
            origin.excluded_publication = Some(observation_id.to_owned());
        };
        if let Some(hold) = state.holds.get_mut(&self.session.provider) {
            rebase(hold);
        }
        if let Some(provisional) = state.provisionals.get_mut(&self.session.provider) {
            rebase(&mut provisional.hold);
        }
        state.publications.insert(
            self.session.provider,
            PositiveReservation {
                publication: publication.clone(),
                carrier,
                active: true,
            },
        );
        Ok(CredentialPositivePublication {
            session: self.session.clone(),
            publication,
            carrier,
        })
    }

    /// Called only after the shared fresh-success protocol settles durable
    /// authority for this target. A stale result removes nothing.
    pub fn retire(self) -> Result<bool, CredentialHoldError> {
        self.session.checkpoint()?;
        let removed = {
            let mut state = self
                .session
                .state
                .holds
                .lock()
                .map_err(|_| CredentialHoldError::Unavailable)?;
            if state.generations[self.session.provider.index()] != self.generation
                || state
                    .provisionals
                    .get(&self.session.provider)
                    .is_some_and(|p| p.serial >= self.serial)
            {
                return Err(CredentialHoldError::Superseded);
            }
            let next_generation = self
                .generation
                .checked_add(1)
                .ok_or(CredentialHoldError::Unavailable)?;
            let matches = self.matched_marker.as_ref().is_some_and(|marker| {
                state.holds.get(&self.session.provider).is_some_and(|hold| {
                    &hold.marker == marker && hold.target_hmac == self.session.target_hmac
                })
            });
            if matches {
                state.holds.remove(&self.session.provider);
            }
            let provisional = state
                .provisionals
                .get(&self.session.provider)
                .is_some_and(|p| {
                    p.hold.target_hmac == self.session.target_hmac && p.serial < self.serial
                });
            if provisional {
                state.provisionals.remove(&self.session.provider);
            }
            // A settled fresh success also invalidates rejection publishers
            // captured before that settlement, even when no hold was matched.
            state.generations[self.session.provider.index()] = next_generation;
            if state
                .publications
                .get(&self.session.provider)
                .is_some_and(|p| {
                    p.publication.serial == self.serial
                        && p.publication.target_hmac == self.session.target_hmac
                })
            {
                state.publications.remove(&self.session.provider);
            }
            matches || provisional
        };
        self.session.registry.sync_pin(&self.session.state)?;
        Ok(removed)
    }
}

fn same_provisional(left: &ProvisionalHold, right: &ProvisionalHold) -> bool {
    left.serial == right.serial
        && left.generation == right.generation
        && left.hold.marker == right.hold.marker
        && left.hold.target_hmac == right.hold.target_hmac
}

/// No filesystem access: IDs come from the caller's strict renewed snapshot.
fn resolve_attempts(state: &mut RootHolds, provider: Provider, ids: &[Option<String>; 2]) {
    let prior = state.attempts.get(&provider).cloned().unwrap_or_default();
    let unresolved = if state.publications.get(&provider).is_some_and(|p| !p.active) {
        state.publications.remove(&provider).map(|p| p.publication)
    } else {
        None
    };
    // Match work identity across BOTH current carriers. Moving an existing
    // partial ID between config and Brain cannot become a fresh observation.
    let proofs = prior
        .into_iter()
        .flatten()
        .chain(unresolved)
        .collect::<Vec<_>>();
    let current = std::array::from_fn(|index| {
        ids[index].as_ref().and_then(|id| {
            proofs
                .iter()
                .find(|proof| &proof.observation_id == id)
                .cloned()
        })
    });
    if current.iter().any(Option::is_some) {
        state.attempts.insert(provider, current);
    } else {
        state.attempts.remove(&provider);
    }
}

impl ProvisionalCredentialRejection {
    pub(crate) fn marker(&self) -> &str {
        &self.provisional.hold.marker
    }

    pub(crate) fn older_publications(&self) -> impl Iterator<Item = &PositivePublication> {
        self.provisional
            .hold
            .origin
            .iter()
            .flat_map(|o| o.older_publications.iter())
    }

    pub(crate) fn ensure_current(&self) -> Result<(), CredentialHoldError> {
        self.session.checkpoint()?;
        let state = self
            .session
            .state
            .holds
            .lock()
            .map_err(|_| CredentialHoldError::Unavailable)?;
        if state.generations[self.session.provider.index()] != self.provisional.generation
            || state
                .provisionals
                .get(&self.session.provider)
                .is_none_or(|p| !same_provisional(p, &self.provisional))
        {
            return Err(CredentialHoldError::Superseded);
        }
        Ok(())
    }

    pub(crate) fn promote(&self) -> Result<VolatileCredentialWitness, CredentialHoldError> {
        self.session.checkpoint()?;
        let generation = {
            let mut state = self
                .session
                .state
                .holds
                .lock()
                .map_err(|_| CredentialHoldError::Unavailable)?;
            if state.generations[self.session.provider.index()] != self.provisional.generation
                || state
                    .provisionals
                    .get(&self.session.provider)
                    .is_none_or(|p| !same_provisional(p, &self.provisional))
            {
                return Err(CredentialHoldError::Superseded);
            }
            let next = self
                .provisional
                .generation
                .checked_add(1)
                .ok_or(CredentialHoldError::Unavailable)?;
            let promoted = state
                .provisionals
                .remove(&self.session.provider)
                .expect("exact provisional still owned");
            state.holds.insert(self.session.provider, promoted.hold);
            state.generations[self.session.provider.index()] = next;
            next
        };
        self.session.registry.sync_pin(&self.session.state)?;
        Ok(VolatileCredentialWitness {
            session: self.session.clone(),
            generation,
            matched_marker: Some(self.marker().to_owned()),
            matched_provisional: None,
            provisional_marker_seen: None,
            serial: self.provisional.serial,
        })
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Journal(PathBuf);
    impl Journal {
        fn new() -> Self {
            let mut id = [0; 16];
            getrandom::fill(&mut id).unwrap();
            let name: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("brain-hold-{name}"));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn managed(
        registry: &Arc<HoldRegistry>,
        journal: &Journal,
        secret: &str,
    ) -> CredentialHoldSession {
        CredentialHoldSession::admit(
            Arc::clone(registry),
            Arc::new(JournalRoot::open(&journal.0).unwrap()),
            CredentialHoldTarget::Managed {
                provider: "openai",
                credential: secret,
            },
        )
        .unwrap()
    }

    fn origin(session: &CredentialHoldSession, config: Option<&str>) -> CredentialHoldOrigin {
        CredentialHoldOrigin {
            config_id: config.map(str::to_owned),
            brain_id: None,
            binding_material_id: Some(session.binding_material_id(&[0x55; 32])),
            older_publications: Vec::new(),
            excluded_publication: None,
        }
    }

    #[test]
    fn provisional_blocks_without_claiming_an_accepted_epoch_and_newest_slot_wins() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let session = managed(&registry, &journal, "key");
        let old = session.validation_witness().unwrap();
        let new = session.validation_witness().unwrap();
        let generation = session.memory_snapshot().unwrap().generation;
        let p = session
            .prepare_rejection(&new, Some(origin(&session, None)))
            .unwrap();
        assert!(session.is_held().unwrap());
        assert_eq!(session.memory_snapshot().unwrap().generation, generation);
        assert!(matches!(
            old.ensure_current(),
            Err(CredentialHoldError::Superseded)
        ));
        assert!(matches!(
            session.prepare_rejection(&old, None),
            Err(CredentialHoldError::Superseded)
        ));
        p.promote().unwrap().ensure_current().unwrap();
        assert_eq!(
            session.memory_snapshot().unwrap().generation,
            generation + 1
        );
    }

    #[test]
    fn partial_positive_without_prior_refusal_survives_drop_and_a_failed_next_attempt() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let session = managed(&registry, &journal, "key");
        let first = session.validation_witness().unwrap();
        let first_id = "a".repeat(64);
        let material = session.binding_material_id(&[0x55; 32]);
        let first_guard = first
            .reserve_positive(
                &first_id,
                None,
                ObservationCarrier::Config,
                &[None, None],
                &material,
            )
            .unwrap();
        drop(first_guard);
        assert!(
            registry.entries.lock().unwrap().pinned.is_empty(),
            "attempts alone never pin a root"
        );
        let ids = [Some(first_id.clone()), None];
        let snapshot = session.memory_snapshot().unwrap();
        let second = session
            .validation_witness_after_snapshot(&snapshot, false, &ids)
            .unwrap();
        let second_guard = second
            .reserve_positive(
                &"b".repeat(64),
                None,
                ObservationCarrier::Config,
                &ids,
                &material,
            )
            .unwrap();
        drop(second_guard); // It never actually replaced first_id.
        let snapshot = session.memory_snapshot().unwrap();
        let negative = session
            .validation_witness_after_snapshot(&snapshot, false, &ids)
            .unwrap();
        let p = session
            .prepare_rejection(&negative, Some(origin(&session, None)))
            .unwrap();
        assert!(
            p.older_publications()
                .any(|proof| proof.observation_id == first_id)
        );
        let snapshot = session.memory_snapshot().unwrap();
        assert!(!snapshot.corrected_by(&session, &[0x55; 32], std::slice::from_ref(&first_id)));
        assert!(snapshot.corrected_by(&session, &[0x55; 32], &["c".repeat(64)]));
        // Moving that same known work ID to Brain cannot manufacture freshness.
        let moved = [None, Some(first_id.clone())];
        let _ = session
            .validation_witness_after_snapshot(&snapshot, false, &moved)
            .unwrap();
        assert!(!session.memory_snapshot().unwrap().corrected_by(
            &session,
            &[0x55; 32],
            &[first_id]
        ));
    }

    #[test]
    fn late_provisional_does_not_absorb_an_already_observed_external_correction() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let session = managed(&registry, &journal, "key");
        let negative = session.validation_witness().unwrap();
        let positive = session.validation_witness().unwrap();
        let p = session
            .prepare_rejection(&negative, Some(origin(&session, Some(&"a".repeat(64)))))
            .unwrap();
        let external = "e".repeat(64);
        let attempt = positive.reserve_positive(
            &"f".repeat(64),
            None,
            ObservationCarrier::Config,
            &[Some(external.clone()), None],
            &session.binding_material_id(&[0x55; 32]),
        );
        assert!(matches!(attempt, Err(CredentialHoldError::Superseded)));
        let snapshot = session.memory_snapshot().unwrap();
        assert!(snapshot.corrected_by(&session, &[0x55; 32], &[external]));
        assert!(session.reconcile(&snapshot).unwrap());
        assert!(!session.is_held().unwrap());
        assert!(matches!(p.promote(), Err(CredentialHoldError::Superseded)));
    }

    #[test]
    fn reconciliation_of_one_target_preserves_the_other_target_provisional() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let a = managed(&registry, &journal, "a");
        let a_before = a.validation_witness().unwrap();
        a.prepare_rejection(&a_before, Some(origin(&a, None)))
            .unwrap()
            .promote()
            .unwrap();
        let b = managed(&registry, &journal, "b");
        let before = b.validation_witness().unwrap();
        let p = b
            .prepare_rejection(&before, Some(origin(&b, None)))
            .unwrap();
        let snapshot = a.memory_snapshot().unwrap();
        assert!(snapshot.corrected_by(&a, &[0x55; 32], &["e".repeat(64)]));
        assert!(a.reconcile(&snapshot).unwrap());
        p.ensure_current().unwrap();
        p.promote().unwrap().ensure_current().unwrap();
        assert!(!a.is_held().unwrap());
        assert!(b.is_held().unwrap());
    }

    #[test]
    fn different_applicable_target_refusal_survives_older_speculative_displacement() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let a = managed(&registry, &journal, "override-a");
        let b = managed(&registry, &journal, "configured-b");
        let a_before = a.validation_witness().unwrap();
        let delayed_b = b.validation_witness().unwrap();
        a.prepare_rejection(&a_before, Some(origin(&a, None)))
            .unwrap();
        let success_b = b.validation_witness().unwrap();
        let _publication = success_b
            .reserve_positive(
                &"b".repeat(64),
                None,
                ObservationCarrier::Config,
                &[None, None],
                &b.binding_material_id(&[0x55; 32]),
            )
            .unwrap();
        b.prepare_rejection(&delayed_b, Some(origin(&b, None)))
            .unwrap();
        success_b.retire().unwrap();
        assert!(
            a.is_held().unwrap(),
            "a completed B owner must not silently discard an applicable learned A rejection"
        );
    }

    #[test]
    fn target_capacity_preserves_all_refusals_and_existing_cell_recovery() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let mut sessions = (0..MAX_LIVE_TARGETS_PER_PROVIDER)
            .map(|i| managed(&registry, &journal, &format!("rejected-{i}")))
            .collect::<Vec<_>>();
        for session in &sessions {
            session.record_rejection().unwrap();
        }
        let newcomer = || {
            CredentialHoldSession::admit(
                registry.clone(),
                Arc::new(JournalRoot::open(&journal.0).unwrap()),
                CredentialHoldTarget::Managed {
                    provider: "openai",
                    credential: "new-correction",
                },
            )
        };
        assert!(matches!(newcomer(), Err(CredentialHoldError::Capacity)));
        for session in &sessions {
            assert!(session.is_held().unwrap());
        }
        // Same actual target can still perform fresh correction at capacity.
        let existing = managed(&registry, &journal, "rejected-0");
        existing.validation_witness().unwrap().retire().unwrap();
        assert!(!existing.is_held().unwrap());
        assert!(
            matches!(newcomer(), Err(CredentialHoldError::Capacity)),
            "an old active witness still owns the cell"
        );
        drop(existing);
        drop(sessions.remove(0));
        assert!(!newcomer().unwrap().is_held().unwrap());
        for session in &sessions {
            assert!(session.is_held().unwrap());
        }
    }

    #[test]
    fn sequential_configured_key_correction_retires_only_quiescent_unselectable_cells() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let root = Arc::new(JournalRoot::open(&journal.0).unwrap());
        for i in 0..(MAX_LIVE_TARGETS_PER_PROVIDER * 3) {
            let key = format!("sequential-{i}");
            solstone_core_journal_config_write::mutate_journal_config_at_root_checked(
                &root,
                Default::default(),
                &|| Ok(()),
                |config| {
                    config.insert("env".to_owned(), serde_json::json!({"OPENAI_API_KEY":key}));
                    solstone_core_journal_config_write::JournalConfigMutation {
                        changed: true,
                        value: (),
                    }
                },
            )
            .unwrap();
            let before = fs::read(journal.0.join("config/journal.json")).unwrap();
            let session = managed(&registry, &journal, &key);
            assert_eq!(
                fs::read(journal.0.join("config/journal.json")).unwrap(),
                before,
                "capacity reclamation never publishes config or provider work"
            );
            session.record_rejection().unwrap();
            assert!(session.is_held().unwrap());
            assert!(
                session.state.owner.targets.lock().unwrap().cells.len()
                    <= MAX_LIVE_TARGETS_PER_PROVIDER
            );
        }
        assert!(!journal.0.join("health/brain.json").exists());
        assert!(!journal.0.join("health/brain-fingerprint.key").exists());
    }

    #[test]
    fn unselectable_retirement_preserves_override_and_every_late_owner_handle() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let a = managed(&registry, &journal, "override-a");
        let b = managed(&registry, &journal, "configured-b");
        let c = managed(&registry, &journal, "old-c");
        for session in [&a, &b, &c] {
            session.record_rejection().unwrap();
        }
        let owner = a.state.owner.clone();
        let config = serde_json::json!({"env":{"OPENAI_API_KEY":"configured-b"}});
        let protected = registry
            .selectable_targets(config.as_object().unwrap(), Some("override-a"))
            .unwrap();
        let late = c.validation_witness().unwrap();
        drop(a);
        drop(b);
        drop(c);
        registry.retire_quiescent_cells(&owner, &protected).unwrap();
        assert_eq!(
            owner.targets.lock().unwrap().pinned.len(),
            3,
            "a pre-work witness prevents retirement even after selection changes"
        );
        let guard = late
            .reserve_positive(
                &"c".repeat(64),
                None,
                ObservationCarrier::Config,
                &[None, None],
                &late.session.binding_material_id(&[0x55; 32]),
            )
            .unwrap();
        drop(late);
        registry.retire_quiescent_cells(&owner, &protected).unwrap();
        assert_eq!(
            owner.targets.lock().unwrap().pinned.len(),
            3,
            "a native publication guard prevents retirement after its witness moves"
        );
        drop(guard);
        registry.retire_quiescent_cells(&owner, &protected).unwrap();
        assert_eq!(owner.targets.lock().unwrap().pinned.len(), 2);
        assert!(
            managed(&registry, &journal, "override-a")
                .is_held()
                .unwrap()
        );
        assert!(
            managed(&registry, &journal, "configured-b")
                .is_held()
                .unwrap()
        );
        assert!(!managed(&registry, &journal, "old-c").is_held().unwrap());
    }

    #[test]
    fn malformed_selectors_never_certify_that_a_refusal_is_unselectable() {
        let registry = HoldRegistry::new([7; 32], 2);
        for value in [
            serde_json::json!({"env":"unreadable"}),
            serde_json::json!({"env":{"OPENAI_API_KEY":42}}),
            serde_json::json!({"providers":{"local":{"credential":{}}}}),
            serde_json::json!({"providers":{"local":false}}),
        ] {
            assert!(matches!(
                registry.selectable_targets(value.as_object().unwrap(), None),
                Err(CredentialHoldError::Capacity)
            ));
        }
        let value = serde_json::json!({"env":{"OPENAI_API_KEY":" configured-b "},
            "providers":{"local":{"endpoint_url":" https://endpoint.invalid/v1/ ",
                "served_model_id":"served","credential":" endpoint-token "}}});
        let protected = registry
            .selectable_targets(value.as_object().unwrap(), Some("override-a"))
            .unwrap();
        assert!(protected.contains(&TargetKey {
            provider: Provider::Openai,
            hmac: managed_credential_binding_hmac(&registry.salt, "openai", "configured-b")
        }));
        assert!(protected.contains(&TargetKey {
            provider: Provider::Openai,
            hmac: managed_credential_binding_hmac(&registry.salt, "openai", "override-a")
        }));
        assert!(protected.contains(&TargetKey {
            provider: Provider::Local,
            hmac: endpoint_credential_binding_hmac(
                &registry.salt,
                "https://endpoint.invalid",
                " endpoint-token "
            )
        }));
    }

    #[test]
    fn empty_target_cells_are_reclaimed_without_evicting_a_held_target() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let held = managed(&registry, &journal, "held");
        held.record_rejection().unwrap();
        for i in 0..64 {
            drop(managed(&registry, &journal, &format!("unused-{i}")));
        }
        assert!(held.is_held().unwrap());
        let targets = held.state.owner.targets.lock().unwrap();
        assert!(targets.cells.len() <= 2);
        assert_eq!(targets.pinned.len(), 1);
    }

    #[test]
    fn hold_survives_fresh_handles_and_only_matches_the_actual_key() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let first = managed(&registry, &journal, "rejected");
        first.record_rejection().unwrap();
        drop(first);
        let next = managed(&registry, &journal, "rejected");
        assert!(next.is_held().unwrap());
        assert!(!managed(&registry, &journal, "corrected").is_held().unwrap());
        assert!(next.validation_witness().unwrap().retire().unwrap());
        assert!(!next.is_held().unwrap());
    }

    #[test]
    fn a_repeated_same_key_rejection_invalidates_older_success() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let session = managed(&registry, &journal, "rejected");
        let before = session.validation_witness().unwrap();
        let first = session.record_rejection().unwrap();
        assert!(matches!(
            before.retire(),
            Err(CredentialHoldError::Superseded)
        ));
        let older = session.validation_witness().unwrap();
        let second = session.record_rejection().unwrap();
        assert_ne!(first, second);
        assert!(matches!(
            older.retire(),
            Err(CredentialHoldError::Superseded)
        ));
        assert!(session.is_held().unwrap());
        assert!(session.validation_witness().unwrap().retire().unwrap());
    }

    #[test]
    fn atomic_registration_witness_is_invalidated_by_a_later_fresh_success() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let session = managed(&registry, &journal, "rejected");
        let before = session.validation_witness().unwrap();
        let (marker, publisher) = session.record_rejection_after(&before).unwrap();
        assert_eq!(publisher.matched_marker.as_deref(), Some(marker.as_str()));
        assert!(session.validation_witness().unwrap().retire().unwrap());
        assert!(matches!(
            publisher.ensure_current(),
            Err(CredentialHoldError::Superseded)
        ));
        assert!(!session.is_held().unwrap());
    }

    #[test]
    fn independent_target_rejections_and_retirement_preserve_the_other_target() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let a = managed(&registry, &journal, "a");
        let before = a.validation_witness().unwrap();
        let b = managed(&registry, &journal, "b");
        b.record_rejection().unwrap();
        let b_before = b.validation_witness().unwrap();
        a.record_rejection_after(&before).unwrap();
        assert!(a.is_held().unwrap());
        assert!(b.is_held().unwrap());
        b_before.ensure_current().unwrap();
        a.validation_witness().unwrap().retire().unwrap();
        assert!(!a.is_held().unwrap());
        assert!(b.is_held().unwrap());
    }

    #[test]
    fn capacity_failure_does_not_evict_a_still_held_root() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 1));
        let journal = Journal::new();
        let session = managed(&registry, &journal, "rejected");
        session.record_rejection().unwrap();
        let other = Journal::new();
        assert!(matches!(
            CredentialHoldSession::admit(
                Arc::clone(&registry),
                Arc::new(JournalRoot::open(&other.0).unwrap()),
                CredentialHoldTarget::Managed {
                    provider: "openai",
                    credential: "other"
                }
            ),
            Err(CredentialHoldError::Capacity)
        ));
        assert!(session.is_held().unwrap());
        session.validation_witness().unwrap().retire().unwrap();
        drop(session);
        assert!(!managed(&registry, &other, "other").is_held().unwrap());
    }

    #[test]
    fn keyless_endpoint_hold_does_not_apply_to_another_endpoint() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let endpoint = |base_url| {
            CredentialHoldSession::admit(
                Arc::clone(&registry),
                Arc::new(JournalRoot::open(&journal.0).unwrap()),
                CredentialHoldTarget::Endpoint {
                    base_url,
                    credential: "",
                },
            )
            .unwrap()
        };
        endpoint("https://one.invalid").record_rejection().unwrap();
        assert!(endpoint("https://one.invalid").is_held().unwrap());
        assert!(!endpoint("https://two.invalid").is_held().unwrap());
    }

    #[test]
    fn replacement_root_is_distinct_and_cannot_retire_the_old_hold() {
        let registry = Arc::new(HoldRegistry::new([7; 32], 2));
        let journal = Journal::new();
        let old = managed(&registry, &journal, "rejected");
        old.record_rejection().unwrap();
        let witness = old.validation_witness().unwrap();
        let displaced = Journal::new();
        fs::remove_dir(&displaced.0).unwrap();
        fs::rename(&journal.0, &displaced.0).unwrap();
        fs::create_dir(&journal.0).unwrap();
        assert!(!managed(&registry, &journal, "rejected").is_held().unwrap());
        assert!(matches!(
            witness.retire(),
            Err(CredentialHoldError::RootChanged)
        ));
        assert_eq!(registry.entries.lock().unwrap().pinned.len(), 1);
    }
}
