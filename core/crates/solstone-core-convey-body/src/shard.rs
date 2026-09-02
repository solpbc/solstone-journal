// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_body_source::{BodyValue, FieldState, ValueState, parse};

/// The native body-value representation used for present normalized values.
pub type NormalizedValue = BodyValue;

/// A permissively decoded normalized shard row.
///
/// Every source field remains absent-tolerant. `import_ids` is empty after shard
/// reading and is populated only by the within-month reader dedupe.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedRow {
    /// Row schema identifier.
    pub schema: FieldState<Value>,
    /// Source family identifier.
    pub source_family: FieldState<Value>,
    /// Source record type.
    pub record_type: FieldState<Value>,
    /// Stable per-record dedupe key.
    pub dedupe_key: FieldState<Value>,
    /// Primary row timestamp.
    pub start_date: FieldState<Value>,
    /// Legacy fallback row timestamp.
    pub start_time: FieldState<Value>,
    /// Source-local day label.
    pub day: FieldState<Value>,
    /// Source record kind.
    pub kind: FieldState<Value>,
    /// Bundle import identifier carried by the row.
    pub import_id: FieldState<Value>,
    /// Normalized month identifier.
    pub month: FieldState<Value>,
    /// End timestamp fallback.
    pub end_date: FieldState<Value>,
    /// Source-side record identifier.
    pub source_record_id: FieldState<Value>,
    /// Source display name.
    pub source_name: FieldState<Value>,
    /// Source version.
    pub source_version: FieldState<Value>,
    /// Measurement unit.
    pub unit: FieldState<Value>,
    /// Normalized source reference.
    pub normalized_ref: FieldState<Value>,
    /// Raw source reference.
    pub raw_ref: FieldState<Value>,
    /// Source metadata object or scalar.
    pub metadata: FieldState<Value>,
    /// Value retaining null, strings, integers, floats, and all other JSON shapes.
    pub value: ValueState,
    /// Import identifiers assembled by within-month dedupe, oldest first.
    pub import_ids: Vec<String>,
    /// All unmodelled JSON fields.
    pub extra: Map<String, Value>,
}

/// Errors raised by the permissive shard reader.
#[derive(Debug)]
pub enum ShardReadError {
    /// The shard could not be read.
    Read {
        /// Shard path.
        path: PathBuf,
        /// I/O failure.
        source: io::Error,
    },
    /// A non-blank JSONL line was not JSON.
    Parse {
        /// Shard path.
        path: PathBuf,
        /// One-based line number.
        line: usize,
        /// JSON parser failure.
        source: serde_json::Error,
    },
    /// A non-blank JSONL line decoded to a non-object JSON value.
    NotAnObject {
        /// Shard path.
        path: PathBuf,
        /// One-based line number.
        line: usize,
    },
}

impl fmt::Display for ShardReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::Parse { path, line, source } => {
                write!(
                    formatter,
                    "could not parse {} line {line}: {source}",
                    path.display()
                )
            }
            Self::NotAnObject { path, line } => {
                write!(
                    formatter,
                    "{} line {line} is not a JSON object",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ShardReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::NotAnObject { .. } => None,
        }
    }
}

impl NormalizedRow {
    /// Returns the first string timestamp among `start_date`, `start_time`, and `end_date`.
    pub fn row_time(&self) -> Option<&str> {
        [&self.start_date, &self.start_time, &self.end_date]
            .into_iter()
            .find_map(field_string)
    }

    pub(crate) fn dedupe_key_text(&self) -> Option<&str> {
        field_string(&self.dedupe_key)
    }

    pub(crate) fn import_id_text(&self) -> Option<&str> {
        field_string(&self.import_id)
    }
}

/// Reads every non-blank JSON object line from one normalized shard.
pub fn read_normalized_shard(path: impl AsRef<Path>) -> Result<Vec<NormalizedRow>, ShardReadError> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).map_err(|source| ShardReadError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let fallback_month = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| is_month(stem))
        .map(str::to_owned);
    let mut rows = Vec::new();
    for (line_index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_number = line_index + 1;
        let value = serde_json::from_str(line).map_err(|source| ShardReadError::Parse {
            path: path.to_path_buf(),
            line: line_number,
            source,
        })?;
        let Value::Object(object) = value else {
            return Err(ShardReadError::NotAnObject {
                path: path.to_path_buf(),
                line: line_number,
            });
        };
        rows.push(normalized_row(object, fallback_month.as_deref()));
    }
    Ok(rows)
}

fn normalized_row(mut object: Map<String, Value>, fallback_month: Option<&str>) -> NormalizedRow {
    let value = match object.remove("value") {
        None => ValueState::Absent,
        Some(value) => ValueState::Present(
            parse(&serde_json::to_vec(&value).expect("JSON value serializes"))
                .expect("JSON value parses into BodyValue"),
        ),
    };
    let mut month = take_field(&mut object, "month");
    if !matches!(&month, FieldState::Present(Value::String(value)) if !value.is_empty())
        && let Some(fallback_month) = fallback_month
    {
        month = FieldState::Present(Value::String(fallback_month.to_owned()));
    }
    NormalizedRow {
        schema: take_field(&mut object, "schema"),
        source_family: take_field(&mut object, "source_family"),
        record_type: take_field(&mut object, "record_type"),
        dedupe_key: take_field(&mut object, "dedupe_key"),
        start_date: take_field(&mut object, "start_date"),
        start_time: take_field(&mut object, "start_time"),
        day: take_field(&mut object, "day"),
        kind: take_field(&mut object, "kind"),
        import_id: take_field(&mut object, "import_id"),
        month,
        end_date: take_field(&mut object, "end_date"),
        source_record_id: take_field(&mut object, "source_record_id"),
        source_name: take_field(&mut object, "source_name"),
        source_version: take_field(&mut object, "source_version"),
        unit: take_field(&mut object, "unit"),
        normalized_ref: take_field(&mut object, "normalized_ref"),
        raw_ref: take_field(&mut object, "raw_ref"),
        metadata: take_field(&mut object, "metadata"),
        value,
        import_ids: Vec::new(),
        extra: object,
    }
}

fn take_field(object: &mut Map<String, Value>, name: &str) -> FieldState<Value> {
    match object.remove(name) {
        None => FieldState::Absent,
        Some(Value::Null) => FieldState::Null,
        Some(value) => FieldState::Present(value),
    }
}

fn field_string(field: &FieldState<Value>) -> Option<&str> {
    match field {
        FieldState::Present(Value::String(value)) => Some(value),
        FieldState::Absent | FieldState::Null | FieldState::Present(_) => None,
    }
}

fn is_month(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(|byte| byte.is_ascii_digit())
        && bytes[5..].iter().all(|byte| byte.is_ascii_digit())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use solstone_core_body_source::BodyValue;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-shard-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn preserves_non_numeric_values_and_all_three_time_sources() {
        let temporary = TempDir::new();
        let shard = temporary.path().join("2024-01.jsonl");
        fs::write(
            &shard,
            concat!(
                r#"{"start_date":"2024-01-01T01:00:00Z","value":"HKCategoryValueSleepAnalysisAsleepCore","stage":"HKCategoryValueSleepAnalysisAsleepCore"}"#, "\n",
                r#"{"start_time":"2024-01-02T01:00:00Z","value":null}"#, "\n",
                r#"{"end_date":"2024-01-03T01:00:00Z","value":"not-a-number"}"#, "\n",
                r#"{"end_date":"2024-01-04T01:00:00Z","value":17}"#, "\n",
                r#"{"end_date":"2024-01-05T01:00:00Z","value":1.25}"#, "\n"
            ),
        )
        .unwrap();
        let rows = read_normalized_shard(&shard).unwrap();
        assert_eq!(rows[0].row_time(), Some("2024-01-01T01:00:00Z"));
        assert_eq!(rows[1].row_time(), Some("2024-01-02T01:00:00Z"));
        assert_eq!(rows[2].row_time(), Some("2024-01-03T01:00:00Z"));
        assert!(matches!(
            &rows[0].value,
            ValueState::Present(BodyValue::String(value))
                if value.code_points().iter().copied().map(char::from_u32).collect::<Option<String>>().as_deref()
                    == Some("HKCategoryValueSleepAnalysisAsleepCore")
        ));
        assert!(matches!(
            &rows[2].value,
            ValueState::Present(BodyValue::String(value))
                if value.code_points().iter().copied().map(char::from_u32).collect::<Option<String>>().as_deref()
                    == Some("not-a-number")
        ));
        assert!(matches!(
            rows[1].value,
            ValueState::Present(BodyValue::Null)
        ));
        assert!(matches!(
            rows[3].value,
            ValueState::Present(BodyValue::Integer(_))
        ));
        assert!(matches!(
            rows[4].value,
            ValueState::Present(BodyValue::Number(_))
        ));
        assert_eq!(
            rows[0].extra["stage"],
            "HKCategoryValueSleepAnalysisAsleepCore"
        );
    }

    #[test]
    fn distinguishes_parse_and_read_failures() {
        let temporary = TempDir::new();
        let shard = temporary.path().join("2024-01.jsonl");
        fs::write(&shard, "{\n").unwrap();
        assert!(matches!(
            read_normalized_shard(&shard),
            Err(ShardReadError::Parse { path, line: 1, .. }) if path == shard
        ));
        fs::remove_file(&shard).unwrap();
        assert!(matches!(
            read_normalized_shard(&shard),
            Err(ShardReadError::Read { path, .. }) if path == shard
        ));
    }
}
