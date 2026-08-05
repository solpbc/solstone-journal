use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde_json::Value;
use solstone_core_journal_io::{PathError, ReadError};

/// Failure while reading or inspecting durable entity-store state.
#[derive(Debug)]
pub enum EntityStoreError {
    Read(ReadError),
    Path(PathError),
    IdentityNotObject {
        path: PathBuf,
    },
    HistoryEventNotObject {
        path: PathBuf,
    },
    PreparedEntityIdMismatch {
        entity_id: String,
        event_entity_id: Option<Value>,
    },
    VisibleEventCollision {
        entity_id: String,
        filename: String,
    },
    HistorySequenceNotInteger,
    InvalidHistoryVersionId,
    RestoreTargetsRecordedMerge,
    RestoreCrossesRecordedMerge,
    AmbiguityInvalidRow {
        path: PathBuf,
        line: usize,
        detail: String,
    },
}

impl fmt::Display for EntityStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::IdentityNotObject { path } => {
                write!(
                    formatter,
                    "entity identity is not an object: {}",
                    path.display()
                )
            }
            Self::HistoryEventNotObject { path } => {
                write!(
                    formatter,
                    "history event is not an object: {}",
                    path.display()
                )
            }
            Self::PreparedEntityIdMismatch {
                entity_id,
                event_entity_id,
            } => write!(
                formatter,
                "prepared history for {entity_id} contains event for {}",
                python_value_display(event_entity_id.as_ref())
            ),
            Self::VisibleEventCollision {
                entity_id,
                filename,
            } => write!(
                formatter,
                "visible history event collision for {entity_id}: {filename}"
            ),
            Self::HistorySequenceNotInteger => {
                formatter.write_str("history event seq must be an integer")
            }
            Self::InvalidHistoryVersionId => {
                formatter.write_str("history event has an invalid version_id")
            }
            Self::RestoreTargetsRecordedMerge => formatter.write_str(
                "generic identity restore cannot target a recorded merge event; \
                 use recorded-merge undo instead",
            ),
            Self::RestoreCrossesRecordedMerge => formatter.write_str(
                "generic identity restore cannot cross a recorded merge event; \
                 use recorded-merge undo instead",
            ),
            Self::AmbiguityInvalidRow { path, line, detail } => write!(
                formatter,
                "entity ambiguities: invalid row {line} in {}: {detail}",
                path.display()
            ),
        }
    }
}

impl Error for EntityStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::IdentityNotObject { .. }
            | Self::HistoryEventNotObject { .. }
            | Self::PreparedEntityIdMismatch { .. }
            | Self::VisibleEventCollision { .. }
            | Self::HistorySequenceNotInteger
            | Self::InvalidHistoryVersionId
            | Self::RestoreTargetsRecordedMerge
            | Self::RestoreCrossesRecordedMerge
            | Self::AmbiguityInvalidRow { .. } => None,
        }
    }
}

impl From<ReadError> for EntityStoreError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<PathError> for EntityStoreError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

fn python_value_display(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::Bool(value)) => {
            if *value {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    }
}
