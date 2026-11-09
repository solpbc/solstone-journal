// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-provided engine authority for platforms that cannot spawn llama-server.
//! Registration carries code already admitted by the app package. Models still
//! use the ordinary pinned installer and its generation-use lease.

use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A request may relinquish its slot before the native generation has joined.
/// The exact authority keeps this carrier until validated retirement; neither
/// an HTTP error nor disappearance from an owner registry confirms it.
#[derive(Default, Debug)]
pub struct InferenceRetirement {
    state: Mutex<InferenceRetirementState>,
}

#[derive(Default, Debug)]
struct InferenceRetirementState {
    requested: bool,
    joined: bool,
    permit: Option<crate::admission::LocalSlotPermit>,
}

impl InferenceRetirement {
    pub fn is_live(&self) -> bool {
        let state = self.state.lock().expect("native inference retirement lock");
        !state.requested && !state.joined
    }

    pub fn requested(&self) -> bool {
        self.state
            .lock()
            .expect("native inference retirement lock")
            .requested
    }

    pub fn request(&self, permit: crate::admission::LocalSlotPermit) {
        let mut state = self.state.lock().expect("native inference retirement lock");
        if !state.joined {
            assert!(
                state.permit.is_none(),
                "one-slot native generation already owns admission"
            );
            state.requested = true;
            state.permit = Some(permit);
        }
        // A previously confirmed join can release this late handoff at once.
    }

    /// The authority calls this only after its owned native join, terminal phase
    /// and generation have all been validated.
    pub fn confirm_joined(&self) {
        let permit = {
            let mut state = self.state.lock().expect("native inference retirement lock");
            state.joined = true;
            state.permit.take()
        };
        drop(permit);
    }
}

// No Debug/serde: the accepted per-launch credential stays in memory.
pub struct InferenceCredentials {
    pub generation: u64,
    pub port: u16,
    pub token: Vec<u8>,
    pub retirement: Arc<InferenceRetirement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub artifact_key: String,
    pub abi: u32,
    pub source_revision: String,
    pub runtime_identity: String,
}

impl RuntimeIdentity {
    pub fn validate(&self) -> Result<(), String> {
        if !supports_key(&self.artifact_key) || self.abi == 0 {
            return Err("the embedded local runtime does not support this platform".into());
        }
        if self.source_revision.len() != 40
            || !self.source_revision.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("the embedded local runtime source identity is invalid".into());
        }
        let digest = self.runtime_identity.strip_prefix("sha256:");
        if !digest
            .is_some_and(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("the embedded local runtime package identity is invalid".into());
        }
        Ok(())
    }

    pub fn backend(&self) -> &'static str {
        if self.artifact_key == "aarch64-ios" {
            "metal"
        } else {
            "cpu"
        }
    }

    pub fn pin(&self) -> Value {
        json!({
            "unit": "llama-embedded",
            "source": "app-package",
            "artifact_key": self.artifact_key,
            "abi": self.abi,
            "source_revision": self.source_revision,
            "runtime_identity": self.runtime_identity,
        })
    }
}

pub fn supports_key(key: &str) -> bool {
    matches!(key, "aarch64-android" | "aarch64-ios")
}

// No Debug: the per-generation credential stays in memory only.
pub struct Launch {
    pub identity: RuntimeIdentity,
    pub generation: u64,
    pub model_generation: String,
    pub model_path: PathBuf,
    pub projector_path: PathBuf,
    pub bearer_token: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Created,
    Loading,
    Ready,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub phase: Phase,
    pub port: Option<NonZeroU16>,
    pub engine_result: i32,
    pub error: String,
}

/// One app-owned worker; Stop never grants authority to signal the host PID.
/// The implementation must retain its handle until complete native retirement.
pub trait Engine: Send {
    fn snapshot(&self) -> Result<Snapshot, String>;
    fn request_stop(&self);
    fn is_finished(&self) -> bool;
    fn stop_and_join(&mut self) -> Result<Snapshot, String>;
}

pub trait Factory: Send + Sync {
    fn identity(&self) -> RuntimeIdentity;
    fn start(&self, launch: Launch) -> Result<Box<dyn Engine>, String>;
}

static FACTORY: OnceLock<Arc<dyn Factory>> = OnceLock::new();

/// Install the app's code authority once, before opening its journal. Reopen of
/// the same package is idempotent; a different image cannot replace a live one.
pub fn register(factory: Arc<dyn Factory>) -> Result<(), String> {
    let identity = factory.identity();
    identity.validate()?;
    if identity.artifact_key != super::install::pins::platform_key() {
        return Err("the embedded local runtime is built for another platform".into());
    }
    match FACTORY.set(factory) {
        Ok(()) => Ok(()),
        Err(factory)
            if FACTORY
                .get()
                .is_some_and(|old| old.identity() == factory.identity()) =>
        {
            Ok(())
        }
        Err(_) => Err("a different embedded local runtime is already registered".into()),
    }
}

pub fn factory() -> Option<Arc<dyn Factory>> {
    FACTORY.get().cloned()
}

pub fn identity_for_key(key: &str) -> Option<RuntimeIdentity> {
    let identity = FACTORY.get()?.identity();
    (identity.artifact_key == key).then_some(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> RuntimeIdentity {
        RuntimeIdentity {
            artifact_key: "aarch64-android".into(),
            abi: 1,
            source_revision: "1".repeat(40),
            runtime_identity: format!("sha256:{}", "2".repeat(64)),
        }
    }

    #[test]
    fn package_identity_refuses_wrong_target_and_malformed_provenance() {
        let valid = identity();
        assert!(valid.validate().is_ok());
        for invalid in [
            RuntimeIdentity {
                artifact_key: "x86_64-unknown-linux-gnu".into(),
                ..valid.clone()
            },
            RuntimeIdentity {
                abi: 0,
                ..valid.clone()
            },
            RuntimeIdentity {
                source_revision: "not-a-revision".into(),
                ..valid.clone()
            },
            RuntimeIdentity {
                runtime_identity: "sha256:missing".into(),
                ..valid
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn package_fingerprint_distinguishes_code_replacement() {
        let old = identity();
        let new = RuntimeIdentity {
            runtime_identity: format!("sha256:{}", "3".repeat(64)),
            ..old.clone()
        };
        assert_ne!(old.pin(), new.pin());
        assert_eq!(old.pin()["source"], "app-package");
    }
}
