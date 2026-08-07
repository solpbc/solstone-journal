// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

const MAX_COMPONENT_BYTES: usize = 93;
const INVALID_COMPONENT: &str = "<invalid>";

#[derive(Clone, PartialEq, Eq)]
enum ComponentState {
    Valid(String),
    Invalid,
}

/// A bounded, redacting locator for a normalized body row.
#[derive(Clone, PartialEq, Eq)]
pub struct Coordinate {
    bundle: ComponentState,
    shard: ComponentState,
    line: u64,
}

impl Coordinate {
    /// Builds a coordinate while eagerly validating and redacting its components.
    pub fn new(bundle: impl AsRef<[u8]>, shard: impl AsRef<[u8]>, line: u64) -> Self {
        Self {
            bundle: validate_component(bundle.as_ref()),
            shard: validate_component(shard.as_ref()),
            line,
        }
    }

    /// Returns the validated bundle component, or a redaction marker.
    pub fn bundle(&self) -> &str {
        rendered_component(&self.bundle)
    }

    /// Returns the validated shard component, or a redaction marker.
    pub fn shard(&self) -> &str {
        rendered_component(&self.shard)
    }

    /// Returns the row line number, or None if the supplied line was invalid (zero).
    pub fn line(&self) -> Option<u64> {
        (self.line != 0).then_some(self.line)
    }
}

impl fmt::Display for Coordinate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}#L", self.bundle(), self.shard())?;
        match self.line() {
            Some(line) => write!(formatter, "{line}"),
            None => write!(formatter, "{INVALID_COMPONENT}"),
        }
    }
}

impl fmt::Debug for Coordinate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Coordinate({self})")
    }
}

fn validate_component(bytes: &[u8]) -> ComponentState {
    if bytes.is_empty() || bytes.len() > MAX_COMPONENT_BYTES {
        return ComponentState::Invalid;
    }
    if bytes == b"." || bytes == b".." {
        return ComponentState::Invalid;
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return ComponentState::Invalid;
    }
    ComponentState::Valid(
        String::from_utf8(bytes.to_vec()).expect("validated ASCII component is valid UTF-8"),
    )
}

fn rendered_component(component: &ComponentState) -> &str {
    match component {
        ComponentState::Valid(value) => value,
        ComponentState::Invalid => INVALID_COMPONENT,
    }
}
