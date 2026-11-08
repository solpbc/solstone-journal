// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a body import offers for its facet, from the owner's remembered
//! default and the journal's declared facets. Read-only: the answer is
//! recorded by `record_body_facet` in the config write crate, and a `body`
//! facet is made with [`crate::create_facet`].

use std::error::Error;
use std::fmt;
use std::path::Path;

use solstone_core_journal_config::{
    BodyFacetOffer, ConfigLoadError, StoredBodyFacet, parse_body_facet, read_journal_config,
};

use crate::{FacetStoreError, observe_declared_facet_inventory};

/// The offer, and whether a stored value had to be ignored to make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyFacetOfferRead {
    pub offer: BodyFacetOffer,
    /// What was wrong with a stored value that is present but not valid. It
    /// was read as never answered, and the file was left as it is.
    pub malformed: Option<String>,
}

/// The configuration or the facet declarations could not be read.
#[derive(Debug)]
pub enum BodyFacetReadError {
    Config(ConfigLoadError),
    Facets(FacetStoreError),
}

impl fmt::Display for BodyFacetReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Facets(error) => error.fmt(formatter),
        }
    }
}

impl Error for BodyFacetReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Facets(error) => Some(error),
        }
    }
}

/// What a body import with the owner present offers for its facet.
pub fn body_facet_offer(journal: &Path) -> Result<BodyFacetOfferRead, BodyFacetReadError> {
    let (stored, enabled) = read_body_facet_basis(journal)?;
    Ok(BodyFacetOfferRead {
        offer: stored.offer(&enabled),
        malformed: stored.malformed().map(str::to_owned),
    })
}

/// The facet a body import with nobody to answer uses: the remembered facet
/// while it is still enabled, otherwise none. Never creates a facet.
pub fn default_for_unattended(journal: &Path) -> Result<Option<String>, BodyFacetReadError> {
    let (stored, enabled) = read_body_facet_basis(journal)?;
    Ok(stored.unattended_default(&enabled))
}

fn read_body_facet_basis(
    journal: &Path,
) -> Result<(StoredBodyFacet, Vec<String>), BodyFacetReadError> {
    let config = read_journal_config(journal)
        .map_err(BodyFacetReadError::Config)?
        .config
        .unwrap_or_default();
    let stored = parse_body_facet(&config);
    if let Some(detail) = stored.malformed() {
        log::warn!("body import facet: ignoring the stored default: {detail}");
    }
    let enabled = observe_declared_facet_inventory(journal)
        .map_err(BodyFacetReadError::Facets)?
        .enabled;
    Ok((stored, enabled))
}
