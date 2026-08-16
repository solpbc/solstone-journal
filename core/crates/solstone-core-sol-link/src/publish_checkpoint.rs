// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Named publication checkpoints for lock-in interruption and crash witnesses.

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishCheckpoint {
    BeforeStagingDirCreate,
    AfterStagingDirCreate,
    MidPopulateCert,
    MidPopulateKey,
    AfterPopulate,
    AfterStagingSync,
    AfterRename,
}

#[allow(dead_code)]
impl PublishCheckpoint {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeStagingDirCreate => "before-staging-dir-create",
            Self::AfterStagingDirCreate => "after-staging-dir-create",
            Self::MidPopulateCert => "mid-populate-cert",
            Self::MidPopulateKey => "mid-populate-key",
            Self::AfterPopulate => "after-populate",
            Self::AfterStagingSync => "after-staging-sync",
            Self::AfterRename => "after-rename",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "before-staging-dir-create" => Some(Self::BeforeStagingDirCreate),
            "after-staging-dir-create" => Some(Self::AfterStagingDirCreate),
            "mid-populate-cert" => Some(Self::MidPopulateCert),
            "mid-populate-key" => Some(Self::MidPopulateKey),
            "after-populate" => Some(Self::AfterPopulate),
            "after-staging-sync" => Some(Self::AfterStagingSync),
            "after-rename" => Some(Self::AfterRename),
            _ => None,
        }
    }
}
