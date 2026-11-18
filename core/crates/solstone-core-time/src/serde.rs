// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `#[serde(with = "…")]` modules that keep the journal's stored and served
//! timestamp text.
//!
//! - [`timestamp`]: an instant as `…Z` with the AutoSi fraction; reads every
//!   form [`crate::parse_datetime`] accepts.
//! - [`zoned`]: a time with its offset, written as RFC 3339 (`Z` for a zero
//!   offset); reads as [`crate::parse_datetime`]. The value is a fixed-offset
//!   zoned time.
//! - [`civil`]: a zone-less date and time, `YYYY-MM-DDTHH:MM:SS[.fraction]`.
//!
//! Each has an `option` submodule for `Option<…>` fields.

use std::fmt;

use serde::Deserializer;
use serde::de::{self, Visitor};

struct TextVisitor<T> {
    expecting: &'static str,
    parse: fn(&str) -> Result<T, crate::ParseError>,
}

impl<T> Visitor<'_> for TextVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
        (self.parse)(value).map_err(E::custom)
    }
}

struct OptionVisitor<T> {
    inner: TextVisitor<T>,
}

impl<'de, T> Visitor<'de> for OptionVisitor<T> {
    type Value = Option<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.inner.expecting)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_str(self.inner).map(Some)
    }
}

macro_rules! text_module {
    ($name:ident, $ty:ty, $expecting:literal, $write:expr, $parse:expr) => {
        #[doc = concat!("Serde for `", stringify!($ty), "`: ", $expecting, ".")]
        pub mod $name {
            use super::TextVisitor;
            use serde::{Deserializer, Serializer};

            /// # Errors
            /// As the serializer reports.
            pub fn serialize<S: Serializer>(value: &$ty, serializer: S) -> Result<S::Ok, S::Error> {
                let write: fn(&$ty) -> String = $write;
                serializer.serialize_str(&write(value))
            }

            /// # Errors
            /// When the text is not an accepted form.
            pub fn deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<$ty, D::Error> {
                deserializer.deserialize_str(TextVisitor {
                    expecting: $expecting,
                    parse: $parse,
                })
            }

            /// The same, for an `Option` field.
            pub mod option {
                use super::super::{OptionVisitor, TextVisitor};
                use serde::{Deserializer, Serializer};

                /// # Errors
                /// As the serializer reports.
                pub fn serialize<S: Serializer>(
                    value: &Option<$ty>,
                    serializer: S,
                ) -> Result<S::Ok, S::Error> {
                    match value {
                        Some(value) => super::serialize(value, serializer),
                        None => serializer.serialize_none(),
                    }
                }

                /// # Errors
                /// When the text is not an accepted form.
                pub fn deserialize<'de, D: Deserializer<'de>>(
                    deserializer: D,
                ) -> Result<Option<$ty>, D::Error> {
                    deserializer.deserialize_option(OptionVisitor {
                        inner: TextVisitor {
                            expecting: $expecting,
                            parse: $parse,
                        },
                    })
                }
            }
        }
    };
}

text_module!(
    timestamp,
    jiff::Timestamp,
    "an RFC 3339 timestamp",
    |value| crate::utc_iso(*value),
    crate::parse::parse_timestamp
);

text_module!(
    zoned,
    jiff::ZonedDateTime,
    "an RFC 3339 timestamp with an offset",
    |value| crate::zoned_rfc3339(value, crate::Precision::AutoSi, true),
    crate::parse_datetime
);

text_module!(
    civil,
    jiff::civil::DateTime,
    "a date and time as YYYY-MM-DDTHH:MM:SS",
    |value| crate::civil_iso(*value),
    crate::parse_civil_datetime
);
