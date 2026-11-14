// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal links: the sealed text that names a place in one journal, and the
//! root secret it is sealed under.
//!
//! A link is a pointer and never a credential. Only the journal that sealed
//! it can open it, opening it writes nothing, and what it opens to is one of
//! a closed set of places that the caller then looks up for itself.

pub mod root;
mod seal;

pub use seal::{MAX_SEALED_CHARS, OpenError, Place, open, seal};
