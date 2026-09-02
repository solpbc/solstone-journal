// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;

use solstone_core_journal_io::operational_log::OplogCatalogError;

#[derive(Debug)]
pub enum CollectError {
    Root,
    Catalog(OplogCatalogError),
    CatalogIo,
    CatalogUtf8,
}

impl fmt::Display for CollectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => formatter.write_str("oplog_catalog_root"),
            Self::Catalog(error) => error.fmt(formatter),
            Self::CatalogIo => formatter.write_str("oplog_catalog_io"),
            Self::CatalogUtf8 => formatter.write_str("oplog_catalog_utf8"),
        }
    }
}

impl Error for CollectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Catalog(error) => Some(error),
            Self::Root | Self::CatalogIo | Self::CatalogUtf8 => None,
        }
    }
}
