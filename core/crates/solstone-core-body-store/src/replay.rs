// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use solstone_core_body_source::{
    BodyEnvelope, BodyLedgerValidator, BodyRowEventError, BodyShardValidator, BundleId,
    EnvelopeError, LedgerEventError, ValidatedBodyLedger, ValidatedBodyShard,
    decode_body_ledger_event, validate_body_row_event,
};

use crate::{BodyDedupeError, BodyDedupeState};

/// Stable category for a body-bundle replay refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BodyBundleReplayErrorKind {
    Location,
    Envelope,
    Ledger,
    Row,
    Dedupe,
}

impl BodyBundleReplayErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Location => "location",
            Self::Envelope => "envelope",
            Self::Ledger => "ledger",
            Self::Row => "row",
            Self::Dedupe => "dedupe",
        }
    }
}

/// A fail-closed error while replaying one physical native body bundle.
#[derive(Clone, PartialEq)]
pub enum BodyBundleReplayError {
    Location { bundle: BundleId, sequence: u64 },
    Envelope(EnvelopeError),
    Ledger(LedgerEventError),
    Row(BodyRowEventError),
    Dedupe(BodyDedupeError),
}

impl BodyBundleReplayError {
    pub fn kind(&self) -> BodyBundleReplayErrorKind {
        match self {
            Self::Location { .. } => BodyBundleReplayErrorKind::Location,
            Self::Envelope(_) => BodyBundleReplayErrorKind::Envelope,
            Self::Ledger(_) => BodyBundleReplayErrorKind::Ledger,
            Self::Row(_) => BodyBundleReplayErrorKind::Row,
            Self::Dedupe(_) => BodyBundleReplayErrorKind::Dedupe,
        }
    }

    pub fn bundle(&self) -> Option<&BundleId> {
        match self {
            Self::Location { bundle, .. } => Some(bundle),
            Self::Envelope(error) => error.bundle(),
            Self::Ledger(error) => error.bundle(),
            Self::Row(error) => Some(error.bundle()),
            Self::Dedupe(error) => Some(error.bundle()),
        }
    }

    pub fn sequence(&self) -> Option<u64> {
        match self {
            Self::Location { sequence, .. } => Some(*sequence),
            Self::Envelope(_) => None,
            Self::Ledger(error) => Some(error.line()),
            Self::Row(error) => Some(error.sequence()),
            Self::Dedupe(error) => Some(error.sequence()),
        }
    }
}

impl fmt::Display for BodyBundleReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Location { bundle, sequence } => write!(
                formatter,
                "body-bundle-replay[{}]#E{sequence} invalid_location",
                bundle.as_str()
            ),
            Self::Envelope(error) => write!(formatter, "body-bundle-replay: {error}"),
            Self::Ledger(error) => write!(formatter, "body-bundle-replay: {error}"),
            Self::Row(error) => write!(formatter, "body-bundle-replay: {error}"),
            Self::Dedupe(error) => write!(formatter, "body-bundle-replay: {error}"),
        }
    }
}

impl fmt::Debug for BodyBundleReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyBundleReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Location { .. } => None,
            Self::Envelope(error) => Some(error),
            Self::Ledger(error) => Some(error),
            Self::Row(error) => Some(error),
            Self::Dedupe(error) => Some(error),
        }
    }
}

/// Transactional replay of exact row/event frame pairs from one checked native bundle.
pub struct BodyBundleReplay<'a> {
    envelope: &'a BodyEnvelope,
    ledger: BodyLedgerValidator<'a>,
    shards: Vec<BodyShardValidator>,
    state: BodyDedupeState,
    events_processed: u64,
    error: Option<BodyBundleReplayError>,
}

impl<'a> BodyBundleReplay<'a> {
    /// Starts a replay. No dedupe state is externally visible until [`Self::finish`].
    pub fn new(envelope: &'a BodyEnvelope) -> Result<Self, BodyBundleReplayError> {
        Self::with_state(envelope, BodyDedupeState::new())
    }

    /// Continues ordered replay by moving prior state behind this fail-closed boundary.
    /// A failure consumes the state; only successful finish returns it.
    pub fn with_state(
        envelope: &'a BodyEnvelope,
        state: BodyDedupeState,
    ) -> Result<Self, BodyBundleReplayError> {
        let mut shards = Vec::with_capacity(envelope.shards().len());
        for index in 0..envelope.shards().len() {
            let index = u64::try_from(index).map_err(|_| BodyBundleReplayError::Location {
                bundle: envelope.bundle_id().clone(),
                sequence: 0,
            })?;
            shards.push(
                BodyShardValidator::new(envelope, index)
                    .map_err(BodyBundleReplayError::Envelope)?,
            );
        }
        Ok(Self {
            envelope,
            ledger: BodyLedgerValidator::new(envelope),
            shards,
            state,
            events_processed: 0,
            error: None,
        })
    }

    /// Replays the next exact normalized-row and ledger-event JSONL frames.
    pub fn push(
        &mut self,
        shard_index: u64,
        row_frame: &[u8],
        ledger_frame: &[u8],
    ) -> Result<(), BodyBundleReplayError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.events_processed >= self.envelope.row_count() {
            let error = BodyBundleReplayError::Location {
                bundle: self.envelope.bundle_id().clone(),
                sequence: self.events_processed.saturating_add(1),
            };
            return Err(self.poison(error));
        }
        let sequence = self.events_processed + 1;

        let event = match decode_body_ledger_event(ledger_frame, self.envelope, sequence) {
            Ok(event) => event,
            Err(error) => return Err(self.poison(BodyBundleReplayError::Ledger(error))),
        };
        let shard = usize::try_from(shard_index)
            .ok()
            .and_then(|index| self.envelope.shards().get(index));
        if shard.is_none_or(|shard| shard.path() != event.shard()) {
            let error = BodyBundleReplayError::Location {
                bundle: self.envelope.bundle_id().clone(),
                sequence,
            };
            return Err(self.poison(error));
        }

        let validated = match validate_body_row_event(self.envelope, row_frame, &event) {
            Ok(validated) => validated,
            Err(error) => return Err(self.poison(BodyBundleReplayError::Row(error))),
        };
        if let Err(error) = self.ledger.push(ledger_frame) {
            return Err(self.poison(BodyBundleReplayError::Ledger(error)));
        }
        let shard_validator = &mut self.shards[shard_index as usize];
        if let Err(error) = shard_validator.push(row_frame) {
            return Err(self.poison(BodyBundleReplayError::Envelope(error)));
        }
        if let Err(error) = self.state.apply(&validated) {
            return Err(self.poison(BodyBundleReplayError::Dedupe(error)));
        }
        self.events_processed = sequence;
        Ok(())
    }

    /// Finishes all physical validators and returns the complete replay result.
    pub fn finish(self) -> Result<ValidatedBodyBundleReplay, BodyBundleReplayError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let mut shard_receipts = Vec::with_capacity(self.shards.len());
        for shard in self.shards {
            shard_receipts.push(shard.finish().map_err(BodyBundleReplayError::Envelope)?);
        }
        let ledger = self
            .ledger
            .finish()
            .map_err(BodyBundleReplayError::Ledger)?;
        Ok(ValidatedBodyBundleReplay {
            bundle_id: self.envelope.bundle_id().clone(),
            event_count: self.events_processed,
            shards: shard_receipts,
            ledger,
            state: self.state,
        })
    }

    fn poison(&mut self, error: BodyBundleReplayError) -> BodyBundleReplayError {
        self.error = Some(error.clone());
        error
    }
}

/// Complete physical-validation and dedupe-replay result for one native bundle.
pub struct ValidatedBodyBundleReplay {
    bundle_id: BundleId,
    event_count: u64,
    shards: Vec<ValidatedBodyShard>,
    ledger: ValidatedBodyLedger,
    state: BodyDedupeState,
}

impl fmt::Debug for ValidatedBodyBundleReplay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "validated-body-bundle-replay[{}]/{}-events",
            self.bundle_id.as_str(),
            self.event_count
        )
    }
}

impl ValidatedBodyBundleReplay {
    pub fn bundle_id(&self) -> &BundleId {
        &self.bundle_id
    }

    pub fn event_count(&self) -> u64 {
        self.event_count
    }

    pub fn shards(&self) -> &[ValidatedBodyShard] {
        &self.shards
    }

    pub fn ledger(&self) -> &ValidatedBodyLedger {
        &self.ledger
    }

    pub fn state(&self) -> &BodyDedupeState {
        &self.state
    }

    pub fn into_state(self) -> BodyDedupeState {
        self.state
    }
}
