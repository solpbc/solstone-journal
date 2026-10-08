// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only mutation of one container member after the manifest is rendered.
//!
//! A release build archives the staged bytes unchanged. Nothing in this module
//! reads the environment or a flag.

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum ContainerSeamKind {
    Drop,
    Grow,
    Flip,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct ContainerSeam {
    pub path: &'static str,
    pub kind: ContainerSeamKind,
}

#[cfg(test)]
std::thread_local! {
    static SEAM: std::cell::Cell<Option<ContainerSeam>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) struct ContainerSeamGuard;

#[cfg(test)]
impl Drop for ContainerSeamGuard {
    fn drop(&mut self) {
        SEAM.with(|cell| cell.set(None));
    }
}

#[cfg(test)]
pub(crate) fn install(seam: ContainerSeam) -> ContainerSeamGuard {
    SEAM.with(|cell| cell.set(Some(seam)));
    ContainerSeamGuard
}

/// `None` omits the member. Any other member is returned unchanged when no
/// seam is installed, which is every release build.
pub(crate) fn apply(dest: &str, bytes: Vec<u8>) -> Option<Vec<u8>> {
    #[cfg(test)]
    {
        let Some(seam) = SEAM.with(|cell| cell.get()) else {
            return Some(bytes);
        };
        if dest != seam.path {
            return Some(bytes);
        }
        match seam.kind {
            ContainerSeamKind::Drop => None,
            ContainerSeamKind::Grow => {
                let mut bytes = bytes;
                bytes.push(0);
                Some(bytes)
            }
            ContainerSeamKind::Flip => {
                let mut bytes = bytes;
                if let Some(last) = bytes.last_mut() {
                    *last ^= 0xff;
                }
                Some(bytes)
            }
        }
    }
    #[cfg(not(test))]
    {
        let _ = dest;
        Some(bytes)
    }
}
