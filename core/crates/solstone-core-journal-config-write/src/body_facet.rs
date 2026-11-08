// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The one writer of the owner's default body-import facet.

use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_config::{
    BODY_FACET_KEY, BodyFacetChoice, IMPORTS_SECTION, StoredBodyFacet, body_facet_value,
    is_body_facet_name, parse_body_facet,
};
use solstone_core_journal_io::LockOptions;

use crate::{ConfigMutationError, JournalConfigMutation, mutate_journal_config};

/// Why the default body-import facet was not recorded.
#[derive(Debug)]
pub enum RecordBodyFacetError {
    /// The chosen facet name cannot be stored as a facet name.
    InvalidFacetName(String),
    /// `imports` holds something other than an object; it is left as it is.
    ImportsNotObject,
    /// The configuration could not be loaded, locked or written.
    Mutation(ConfigMutationError),
}

impl fmt::Display for RecordBodyFacetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFacetName(name) => {
                write!(formatter, "{name:?} is not a facet name")
            }
            Self::ImportsNotObject => write!(
                formatter,
                "{IMPORTS_SECTION} in config/journal.json is not an object; it was not changed"
            ),
            Self::Mutation(error) => error.fmt(formatter),
        }
    }
}

impl Error for RecordBodyFacetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Mutation(error) => Some(error),
            _ => None,
        }
    }
}

/// Record the owner's answer to the default body-import facet question.
///
/// This is the owner's default, not one import's facet: an import that picks a
/// facet for itself passes it as that import's argument and never calls this.
/// It does not create the facet. Every other key in `config/journal.json`,
/// including the rest of `imports`, is kept as it was. Returns what was stored
/// before, so a malformed value this answer replaces can be reported.
pub fn record_body_facet(
    journal: &Path,
    choice: &BodyFacetChoice,
) -> Result<StoredBodyFacet, RecordBodyFacetError> {
    if let BodyFacetChoice::Facet(name) = choice
        && !is_body_facet_name(name)
    {
        return Err(RecordBodyFacetError::InvalidFacetName(name.clone()));
    }
    let value = body_facet_value(choice);
    let transaction = mutate_journal_config(journal, LockOptions::default(), |config| {
        let previous = parse_body_facet(config);
        let imports = config
            .entry(IMPORTS_SECTION)
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(imports) = imports.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err(RecordBodyFacetError::ImportsNotObject),
            };
        };
        let changed = imports.get(BODY_FACET_KEY) != Some(&value);
        if changed {
            imports.insert(BODY_FACET_KEY.to_owned(), value);
        }
        JournalConfigMutation {
            changed,
            value: Ok(previous),
        }
    })
    .map_err(RecordBodyFacetError::Mutation)?;
    transaction.value
}

#[cfg(test)]
mod tests {
    use std::fs;

    use solstone_core_journal_config::{get_journal_config_path, read_journal_config};

    use super::*;
    use crate::test_support::TempDir;

    fn write_config(journal: &Path, bytes: &[u8]) {
        let path = get_journal_config_path(journal);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn config_bytes(journal: &Path) -> Vec<u8> {
        fs::read(get_journal_config_path(journal)).unwrap()
    }

    fn stored(journal: &Path) -> StoredBodyFacet {
        parse_body_facet(&read_journal_config(journal).unwrap().config.unwrap())
    }

    fn body() -> BodyFacetChoice {
        BodyFacetChoice::Facet("body".to_owned())
    }

    #[test]
    fn recorded_choices_read_back() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        write_config(journal, b"{}\n");
        assert_eq!(
            record_body_facet(journal, &body()).unwrap(),
            StoredBodyFacet::Unanswered
        );
        assert_eq!(stored(journal), StoredBodyFacet::Answered(body()));
        assert_eq!(
            record_body_facet(journal, &BodyFacetChoice::None).unwrap(),
            StoredBodyFacet::Answered(body())
        );
        assert_eq!(
            stored(journal),
            StoredBodyFacet::Answered(BodyFacetChoice::None)
        );
    }

    /// Other sections, and the rest of `imports`, keep their exact bytes.
    #[test]
    fn other_sections_are_kept_byte_for_byte() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        let before = "{\n  \"identity\": {\n    \"name\": \"Synthetic Owner\",\n    \"aliases\": [\n      \"ö\",\n      \"x\"\n    ]\n  },\n  \"imports\": {\n    \"other\": {\n      \"kept\": 0.1\n    }\n  },\n  \"retention\": {\n    \"raw_media\": \"keep\"\n  },\n  \"zz_unknown\": [\n    1,\n    2.5,\n    null\n  ]\n}\n";
        write_config(journal, before.as_bytes());

        record_body_facet(journal, &body()).unwrap();
        let expected = before.replace(
            "    \"other\": {\n      \"kept\": 0.1\n    }\n",
            "    \"other\": {\n      \"kept\": 0.1\n    },\n    \"body_facet\": {\n      \"choice\": \"facet\",\n      \"facet\": \"body\"\n    }\n",
        );
        assert_eq!(String::from_utf8(config_bytes(journal)).unwrap(), expected);

        record_body_facet(journal, &BodyFacetChoice::None).unwrap();
        let expected = before.replace(
            "    \"other\": {\n      \"kept\": 0.1\n    }\n",
            "    \"other\": {\n      \"kept\": 0.1\n    },\n    \"body_facet\": {\n      \"choice\": \"none\"\n    }\n",
        );
        assert_eq!(String::from_utf8(config_bytes(journal)).unwrap(), expected);
    }

    #[test]
    fn a_new_imports_section_leaves_the_rest_unchanged() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        let before = "{\n  \"retention\": {\n    \"raw_media\": \"keep\"\n  }\n}\n";
        write_config(journal, before.as_bytes());
        record_body_facet(journal, &BodyFacetChoice::None).unwrap();
        assert_eq!(
            String::from_utf8(config_bytes(journal)).unwrap(),
            "{\n  \"retention\": {\n    \"raw_media\": \"keep\"\n  },\n  \"imports\": {\n    \"body_facet\": {\n      \"choice\": \"none\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn the_same_answer_again_does_not_rewrite_the_file() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        let before = "{\"imports\":{\"body_facet\":{\"choice\":\"none\"}}}";
        write_config(journal, before.as_bytes());
        record_body_facet(journal, &BodyFacetChoice::None).unwrap();
        assert_eq!(config_bytes(journal), before.as_bytes());
    }

    #[test]
    fn an_answer_replacing_a_malformed_value_reports_it() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        write_config(journal, br#"{"imports":{"body_facet":"body"}}"#);
        let previous = record_body_facet(journal, &body()).unwrap();
        assert!(previous.malformed().is_some(), "{previous:?}");
        assert_eq!(stored(journal), StoredBodyFacet::Answered(body()));
    }

    #[test]
    fn imports_that_is_not_an_object_is_refused_and_kept() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        let before = br#"{"imports":["kept"]}"#;
        write_config(journal, before);
        assert!(matches!(
            record_body_facet(journal, &body()),
            Err(RecordBodyFacetError::ImportsNotObject)
        ));
        assert_eq!(config_bytes(journal), before);
    }

    #[test]
    fn an_invalid_facet_name_is_refused_before_any_write() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        for name in ["", " body", "a/b", ".."] {
            assert!(matches!(
                record_body_facet(journal, &BodyFacetChoice::Facet(name.to_owned())),
                Err(RecordBodyFacetError::InvalidFacetName(_))
            ));
        }
        assert!(!get_journal_config_path(journal).exists());
    }

    #[test]
    fn a_corrupt_config_is_not_written() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        write_config(journal, b"{not json");
        assert!(matches!(
            record_body_facet(journal, &body()),
            Err(RecordBodyFacetError::Mutation(ConfigMutationError::Load(_)))
        ));
        assert_eq!(config_bytes(journal), b"{not json");
    }
}
