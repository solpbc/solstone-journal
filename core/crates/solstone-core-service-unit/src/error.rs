// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use unicode_general_category::{GeneralCategory, get_general_category};

/// Why a journal path cannot safely be embedded in a managed wrapper target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalPathRejection {
    /// A character forbidden by the retained wrapper format.
    LegacyReserved { character: char },
    /// A Unicode scalar whose general category is unsafe in a line-oriented unit file.
    ForbiddenCategory {
        character: char,
        category: &'static str,
    },
}

/// A failure while rendering a service definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceUnitError {
    /// The journal path cannot safely be embedded in the rendered service.
    InvalidJournalPath(JournalPathRejection),
}

impl fmt::Display for JournalPathRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LegacyReserved { character } => write!(
                formatter,
                "journal path contains legacy-reserved character {character:?}"
            ),
            Self::ForbiddenCategory {
                character,
                category,
            } => write!(
                formatter,
                "journal path contains forbidden Unicode category {category} character {character:?}"
            ),
        }
    }
}

impl fmt::Display for ServiceUnitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJournalPath(reason) => reason.fmt(formatter),
        }
    }
}

impl std::error::Error for ServiceUnitError {}

pub(crate) fn validate_journal_path(journal_path: &str) -> Result<(), ServiceUnitError> {
    for character in journal_path.chars() {
        if matches!(character, '$' | '`' | '"' | '\\' | '\n') {
            return Err(ServiceUnitError::InvalidJournalPath(
                JournalPathRejection::LegacyReserved { character },
            ));
        }
    }
    for character in journal_path.chars() {
        let category = get_general_category(character);
        let category = match category {
            GeneralCategory::Control => Some("Cc"),
            GeneralCategory::Format => Some("Cf"),
            GeneralCategory::LineSeparator => Some("Zl"),
            GeneralCategory::ParagraphSeparator => Some("Zp"),
            _ => None,
        };
        if let Some(category) = category {
            return Err(ServiceUnitError::InvalidJournalPath(
                JournalPathRejection::ForbiddenCategory {
                    character,
                    category,
                },
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{JournalPathRejection, ServiceUnitError, validate_journal_path};

    #[test]
    fn legacy_characters_take_precedence_over_categories() {
        assert_eq!(
            validate_journal_path("journal\n").unwrap_err(),
            ServiceUnitError::InvalidJournalPath(JournalPathRejection::LegacyReserved {
                character: '\n',
            })
        );
    }

    #[test]
    fn rejects_required_unicode_categories() {
        for (character, category) in [
            ('\u{1}', "Cc"),
            ('\u{ad}', "Cf"),
            ('\u{2028}', "Zl"),
            ('\u{2029}', "Zp"),
        ] {
            assert_eq!(
                validate_journal_path(&format!("journal{character}")).unwrap_err(),
                ServiceUnitError::InvalidJournalPath(JournalPathRejection::ForbiddenCategory {
                    character,
                    category,
                })
            );
        }
    }
}
