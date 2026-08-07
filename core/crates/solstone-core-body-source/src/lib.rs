// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible JSON source decoding for body import data.

mod error;
mod integer;
mod parser;
mod string;
mod value;

pub use error::ParseError;
pub use integer::BodyInteger;
pub use parser::parse;
pub use string::BodyString;
pub use value::{BodyObject, BodyValue};
