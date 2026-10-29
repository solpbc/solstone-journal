// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::journal_root::JournalEntryKind;

/// Lossless no-follow entry observation. Equality includes native change stamps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaEntry {
    pub name: OsString,
    pub kind: JournalEntryKind,
    pub identity: MediaIdentity,
    pub size: u64,
    pub(super) stamp: platform::Stamp,
}

impl MediaEntry {
    /// A descriptive change revision, including the private native stamp.
    /// Comparing it supplies no IO or settlement capability.
    pub fn observation_revision(&self) -> io::Result<String> {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "name":self.name.to_str().ok_or_else(changed)?,
            "kind":format!("{:?}",self.kind),"identity":self.identity,
            "size":self.size,"native_stamp":self.stamp,
        }))
        .map_err(io::Error::other)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaRead {
    pub entry: MediaEntry,
    pub bytes: Vec<u8>,
    pub complete: bool,
}

impl MediaDirectory {
    /// Existing-only child admission retaining the entire actual parent chain.
    pub fn open_child(&self, name: &OsStr) -> io::Result<Option<Arc<Self>>> {
        plain_name(name)?;
        self.revalidate()?;
        let child = self
            .inner
            .open_child(name)?
            .map(|inner| Arc::new(Self { inner }));
        self.revalidate()?;
        Ok(child)
    }

    /// Two guarded complete walks. None means the limit was exceeded, never
    /// empty coverage. This does not freeze writers or mint destructive authority.
    pub fn entries(&self, maximum: usize) -> io::Result<Option<Vec<MediaEntry>>> {
        self.listed(maximum, platform::entries)
    }

    /// `entries`, keeping UTF-8 names the portable rules refuse, such as old
    /// logs named with a colon. A name that is not UTF-8 still refuses. Only
    /// for an observer that classifies every name itself: such a name still
    /// cannot be opened or read through this type.
    pub fn entries_keeping_any_name(&self, maximum: usize) -> io::Result<Option<Vec<MediaEntry>>> {
        self.listed(maximum, platform::entries_keeping_any_name)
    }

    fn listed(
        &self,
        maximum: usize,
        list: fn(&platform::Directory, usize) -> io::Result<Option<Vec<MediaEntry>>>,
    ) -> io::Result<Option<Vec<MediaEntry>>> {
        self.revalidate()?;
        let before = self.inner.stamp()?;
        let Some(first) = list(&self.inner, maximum)? else {
            self.revalidate()?;
            return Ok(None);
        };
        step(MediaPrimitive::AfterFirstEnumeration)?;
        self.revalidate()?;
        if self.inner.stamp()? != before {
            return Err(changed());
        }
        let Some(second) = list(&self.inner, maximum)? else {
            return Err(changed());
        };
        self.revalidate()?;
        if first != second || self.inner.stamp()? != before {
            return Err(changed());
        }
        Ok(Some(second))
    }

    /// Read a bounded prefix, allowing files larger than the prefix budget.
    pub fn read_prefix(&self, name: &OsStr, maximum: usize) -> io::Result<Option<MediaRead>> {
        self.read_observed(name, maximum, false)
    }

    /// Read complete bytes, refusing oversize files before allocating a buffer.
    pub fn read_file_bounded(&self, name: &OsStr, maximum: usize) -> io::Result<Option<MediaRead>> {
        self.read_observed(name, maximum, true)
    }

    fn read_observed(
        &self,
        name: &OsStr,
        maximum: usize,
        whole: bool,
    ) -> io::Result<Option<MediaRead>> {
        plain_name(name)?;
        self.revalidate()?;
        let Some(mut file) = platform::open_source(&self.inner, name)? else {
            self.revalidate()?;
            return Ok(None);
        };
        let entry = platform::entry_from_file(name, &file)?;
        if whole && u128::from(entry.size) > maximum as u128 {
            return Err(invalid("observed file exceeds byte limit"));
        }
        let length = usize::try_from(u128::from(entry.size).min(maximum as u128))
            .map_err(|_| invalid("observed prefix exceeds address space"))?;
        let mut bytes = vec![0; length];
        step(MediaPrimitive::BeforePrefixRead)?;
        file.read_exact(&mut bytes).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                changed()
            } else {
                error
            }
        })?;
        step(MediaPrimitive::AfterPrefixRead)?;
        if platform::entry_from_file(name, &file)? != entry
            || platform::entry(&self.inner, name)? != Some(entry.clone())
        {
            return Err(changed());
        }
        self.revalidate()?;
        Ok(Some(MediaRead {
            complete: length as u128 == u128::from(entry.size),
            entry,
            bytes,
        }))
    }
}
