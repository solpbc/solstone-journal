// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Caller-owned Windows publication-path admission and retained ancestor chain.
//!
//! The lexical parser is host-neutral so AC1 runs on a Unix build host. The
//! retained capability and prepare/revalidate path are Windows-only. A
//! current-directory-relative spec is resolved to drive-absolute (cwd
//! components plus caller components) before any handle is opened; the retained
//! chain always starts at the drive root and covers every ancestor through the
//! terminal parent.

#![cfg_attr(
    not(windows),
    allow(dead_code, reason = "consumed by Windows prepare and host unit tests")
)]

use std::error::Error;
use std::fmt;

use crate::name_admission::{NameAdmissionReason, check_portable_component};

/// Maximum extended-length path in UTF-16 code units, excluding the terminating NUL.
///
/// Microsoft documents `\\?` / `\\?\Volume{GUID}\` paths as supporting "a maximum
/// total path length of 32,767 characters":
/// <https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation>
const MAX_EXTENDED_PATH_UTF16: usize = 32_767;

const VOLUME_GUID_PREFIX: [u16; 11] = [
    b'\\' as u16,
    b'\\' as u16,
    b'?' as u16,
    b'\\' as u16,
    b'V' as u16,
    b'o' as u16,
    b'l' as u16,
    b'u' as u16,
    b'm' as u16,
    b'e' as u16,
    b'{' as u16,
];

/// Why a publication path string is not admissible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicationPathParseError {
    Empty,
    RootOnly,
    DriveRelative,
    RootedWithoutDrive,
    UncOrDevicePrefix,
    DotComponent,
    InteriorNul,
    InvalidComponent(NameAdmissionReason),
}

impl fmt::Display for PublicationPathParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("publication path is empty"),
            Self::RootOnly => {
                formatter.write_str("publication path is a drive root with no destination")
            }
            Self::DriveRelative => formatter.write_str("publication path is drive-relative"),
            Self::RootedWithoutDrive => {
                formatter.write_str("publication path is rooted without a drive")
            }
            Self::UncOrDevicePrefix => {
                formatter.write_str("publication path has a UNC or device prefix")
            }
            Self::DotComponent => formatter.write_str("publication path contains '.' or '..'"),
            Self::InteriorNul => formatter.write_str("publication path contains an interior NUL"),
            Self::InvalidComponent(reason) => {
                write!(
                    formatter,
                    "publication path component is not portable: {reason}"
                )
            }
        }
    }
}

impl Error for PublicationPathParseError {}

/// Admitted publication-path components. The last component is the destination leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublicationPathSpec {
    CurrentDirectoryRelative { components: Vec<String> },
    DriveAbsolute { drive: u8, components: Vec<String> },
}

impl PublicationPathSpec {
    fn components(&self) -> &[String] {
        match self {
            Self::CurrentDirectoryRelative { components }
            | Self::DriveAbsolute { components, .. } => components,
        }
    }

    fn ancestors(&self) -> &[String] {
        let components = self.components();
        &components[..components.len() - 1]
    }

    fn leaf(&self) -> &str {
        self.components()
            .last()
            .expect("parse_publication_path requires a leaf")
    }
}

/// Admit a caller-owned publication path using Windows path syntax on every host.
pub(crate) fn parse_publication_path(
    input: &str,
) -> Result<PublicationPathSpec, PublicationPathParseError> {
    if input.is_empty() {
        return Err(PublicationPathParseError::Empty);
    }
    if input.contains('\0') {
        return Err(PublicationPathParseError::InteriorNul);
    }
    if starts_with_two_separators(input) {
        return Err(PublicationPathParseError::UncOrDevicePrefix);
    }
    if let Some(drive) = ascii_drive_letter(input) {
        let remainder = &input[2..];
        let Some(first) = remainder.chars().next() else {
            return Err(PublicationPathParseError::DriveRelative);
        };
        if !is_separator(first) {
            return Err(PublicationPathParseError::DriveRelative);
        }
        let remainder = trim_leading_separators(remainder);
        if remainder.is_empty() {
            return Err(PublicationPathParseError::RootOnly);
        }
        return Ok(PublicationPathSpec::DriveAbsolute {
            drive,
            components: split_components(remainder)?,
        });
    }
    if input.chars().next().is_some_and(is_separator) {
        return Err(PublicationPathParseError::RootedWithoutDrive);
    }
    Ok(PublicationPathSpec::CurrentDirectoryRelative {
        components: split_components(input)?,
    })
}

fn is_separator(character: char) -> bool {
    character == '/' || character == '\\'
}

fn starts_with_two_separators(input: &str) -> bool {
    let mut characters = input.chars();
    match (characters.next(), characters.next()) {
        (Some(first), Some(second)) => is_separator(first) && is_separator(second),
        _ => false,
    }
}

fn ascii_drive_letter(input: &str) -> Option<u8> {
    let bytes = input.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        Some(bytes[0].to_ascii_uppercase())
    } else {
        None
    }
}

fn trim_leading_separators(input: &str) -> &str {
    input.trim_start_matches(['/', '\\'])
}

fn split_components(input: &str) -> Result<Vec<String>, PublicationPathParseError> {
    let mut components = Vec::new();
    let mut current = String::new();
    let mut in_component = false;
    for character in input.chars() {
        if is_separator(character) {
            if !in_component {
                return Err(PublicationPathParseError::InvalidComponent(
                    NameAdmissionReason::Empty,
                ));
            }
            push_component(&mut components, &current)?;
            current.clear();
            in_component = false;
        } else {
            current.push(character);
            in_component = true;
        }
    }
    if !in_component {
        return Err(PublicationPathParseError::InvalidComponent(
            NameAdmissionReason::Empty,
        ));
    }
    push_component(&mut components, &current)?;
    Ok(components)
}

fn push_component(
    components: &mut Vec<String>,
    component: &str,
) -> Result<(), PublicationPathParseError> {
    if component == "." || component == ".." {
        return Err(PublicationPathParseError::DotComponent);
    }
    check_portable_component(component).map_err(PublicationPathParseError::InvalidComponent)?;
    components.push(component.to_owned());
    Ok(())
}

/// Joins a Mount-Manager volume-GUID prefix with ancestor names (no leaf).
fn join_guid_parent(anchor_guid: &[u16], ancestors: &[&str]) -> Vec<u16> {
    if ancestors.is_empty() {
        return anchor_guid.to_vec();
    }
    let prefix = strip_trailing_separators(anchor_guid);
    let mut out = prefix.to_vec();
    for ancestor in ancestors {
        out.push(b'\\' as u16);
        out.extend(ancestor.encode_utf16());
    }
    out
}

fn strip_trailing_separators(buffer: &[u16]) -> &[u16] {
    let mut end = buffer.len();
    while end > 0 {
        let last = buffer[end - 1];
        if last == u16::from(b'\\') || last == u16::from(b'/') {
            end -= 1;
        } else {
            break;
        }
    }
    &buffer[..end]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MoveSpellingTooLong;

/// Refuse a volume-GUID parent spelling that would exceed the extended-length bound.
fn check_move_spelling_budget(
    guid_prefix: &[u16],
    ancestors: &[&str],
) -> Result<(), MoveSpellingTooLong> {
    let mut length = if ancestors.is_empty() {
        guid_prefix.len()
    } else {
        strip_trailing_separators(guid_prefix).len()
    };
    for ancestor in ancestors {
        length = length.saturating_add(1);
        length = length.saturating_add(ancestor.encode_utf16().count());
        if length > MAX_EXTENDED_PATH_UTF16 {
            return Err(MoveSpellingTooLong);
        }
    }
    if length > MAX_EXTENDED_PATH_UTF16 {
        Err(MoveSpellingTooLong)
    } else {
        Ok(())
    }
}

/// Budget each terminal name as a sibling under `ancestors`, not a chained path.
fn check_terminal_names_budget(
    guid_prefix: &[u16],
    ancestors: &[&str],
    names: &[&str],
) -> Result<(), MoveSpellingTooLong> {
    for name in names {
        let mut components = Vec::with_capacity(ancestors.len() + 1);
        components.extend_from_slice(ancestors);
        components.push(name);
        check_move_spelling_budget(guid_prefix, &components)?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) use windows_impl::{
    PublicationPathError, WindowsPublicationPath, prepare_publication_path_with_terminals,
};

#[cfg(windows)]
mod windows_impl {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::mem::size_of;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::path::{Path, PathBuf};

    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_DIRECTORY, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND,
        ERROR_PATH_NOT_FOUND, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, FileAttributeTagInfo, GetFileInformationByHandleEx,
        GetFinalPathNameByHandleW, OPEN_EXISTING, SYNCHRONIZE, VOLUME_NAME_GUID,
    };

    use super::{
        MAX_EXTENDED_PATH_UTF16, PublicationPathParseError, PublicationPathSpec,
        VOLUME_GUID_PREFIX, ascii_drive_letter, check_terminal_names_budget, join_guid_parent,
        parse_publication_path,
    };
    use crate::windows_identity::{WindowsFileIdentity, file_identity};
    use crate::windows_ntcreate::nt_create_relative_deny_delete_sharing;

    const DIRECTORY_ACCESS: u32 =
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE | SYNCHRONIZE;
    const DIRECTORY_OPTIONS: u32 = FILE_DIRECTORY_FILE
        | FILE_OPEN_FOR_BACKUP_INTENT
        | FILE_OPEN_REPARSE_POINT
        | FILE_SYNCHRONOUS_IO_NONALERT;
    const ANCHOR_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
    const PROBE_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    const GUID_PATH_INITIAL_UNITS: usize = 512;

    /// Failure while preparing or revalidating a retained publication path.
    #[derive(Debug)]
    pub(crate) enum PublicationPathError {
        Parse(PublicationPathParseError),
        UnstableNamespace {
            source: io::Error,
        },
        PathTooLong,
        Io {
            operation: &'static str,
            source: io::Error,
        },
        NotDirectory {
            component: String,
        },
        ReparsePoint {
            component: String,
        },
        AlreadyExists {
            component: String,
        },
        IdentityMismatch {
            component: String,
        },
        IdentityChanged {
            level: usize,
        },
        MoveSpellingDiverged,
    }

    impl std::fmt::Display for PublicationPathError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Parse(error) => error.fmt(formatter),
                Self::UnstableNamespace { source } => {
                    write!(
                        formatter,
                        "publication path has no Mount-Manager volume GUID: {source}"
                    )
                }
                Self::PathTooLong => {
                    formatter.write_str("publication path exceeds the extended-length bound")
                }
                Self::Io { operation, source } => {
                    write!(formatter, "{operation}: {source}")
                }
                Self::NotDirectory { component } => {
                    write!(
                        formatter,
                        "publication path ancestor '{component}' is not a directory"
                    )
                }
                Self::ReparsePoint { component } => {
                    write!(
                        formatter,
                        "publication path ancestor '{component}' is a reparse point"
                    )
                }
                Self::AlreadyExists { component } => {
                    write!(
                        formatter,
                        "publication path ancestor '{component}' appeared during create-only"
                    )
                }
                Self::IdentityMismatch { component } => {
                    write!(
                        formatter,
                        "publication path ancestor '{component}' identity changed between create and reopen"
                    )
                }
                Self::IdentityChanged { level } => {
                    write!(
                        formatter,
                        "publication path identity changed at retained level {level}"
                    )
                }
                Self::MoveSpellingDiverged => formatter.write_str(
                    "publication path move spelling no longer names the terminal parent",
                ),
            }
        }
    }

    impl std::error::Error for PublicationPathError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::Parse(error) => Some(error),
                Self::UnstableNamespace { source } | Self::Io { source, .. } => Some(source),
                Self::PathTooLong
                | Self::NotDirectory { .. }
                | Self::ReparsePoint { .. }
                | Self::AlreadyExists { .. }
                | Self::IdentityMismatch { .. }
                | Self::IdentityChanged { .. }
                | Self::MoveSpellingDiverged => None,
            }
        }
    }

    /// Retained no-follow ancestor chain for a later create-only publication writer.
    /// Always starts at the opened drive root and includes every ancestor through
    /// the terminal parent.
    ///
    /// Not `Clone`: the OS share-mode guarantee is bound to these live handles.
    pub(crate) struct WindowsPublicationPath {
        retained: Vec<(OwnedHandle, WindowsFileIdentity)>,
        #[allow(
            dead_code,
            reason = "diagnostic spelling retained for callers; create-only reports the caller path"
        )]
        diagnostic_path: PathBuf,
        leaf: OsString,
        move_spelling: OsString,
    }

    #[cfg(windows)]
    const _: fn() = || {
        trait AmbiguousIfClone<A> {
            fn confirm_not_clone() {}
        }
        impl<T: ?Sized> AmbiguousIfClone<()> for T {}
        struct ImplementsClone;
        impl<T: ?Sized + Clone> AmbiguousIfClone<ImplementsClone> for T {}
        let _ = <WindowsPublicationPath as AmbiguousIfClone<_>>::confirm_not_clone;
    };

    impl WindowsPublicationPath {
        /// Diagnostic path only; it is never authority for a child operation.
        #[allow(
            dead_code,
            reason = "diagnostic spelling retained for callers; create-only reports the caller path"
        )]
        pub(crate) fn diagnostic_path(&self) -> &Path {
            &self.diagnostic_path
        }

        /// Retained terminal-parent handle used as the authority for the leaf.
        pub(crate) fn terminal_parent(&self) -> &OwnedHandle {
            &self
                .retained
                .last()
                .expect("prepare always retains the terminal parent")
                .0
        }

        /// Admitted destination leaf name; `WindowsPublicationPath` itself never opens or creates it —
        /// callers publish onto it via a no-replace move.
        pub(crate) fn leaf_name(&self) -> &OsStr {
            &self.leaf
        }

        /// Volume-GUID absolute spelling of the terminal parent (drive-root GUID plus every ancestor, no leaf).
        pub(crate) fn move_spelling(&self) -> &OsStr {
            &self.move_spelling
        }

        /// Re-check every retained identity (drive root through terminal parent) and that `move_spelling` still names the parent.
        pub(crate) fn revalidate(&self) -> Result<(), PublicationPathError> {
            for (level, (handle, frozen)) in self.retained.iter().enumerate() {
                let observed = validate_retained_directory(handle.as_raw_handle(), "<retained>")?;
                if observed != *frozen {
                    return Err(PublicationPathError::IdentityChanged { level });
                }
            }
            let terminal = self
                .retained
                .last()
                .expect("prepare always retains the terminal parent");
            let spelling = open_move_spelling(&self.move_spelling)?;
            let observed =
                validate_retained_directory(spelling.as_raw_handle(), "<move-spelling>")?;
            if observed != terminal.1 {
                return Err(PublicationPathError::MoveSpellingDiverged);
            }
            Ok(())
        }

        #[cfg(test)]
        pub(crate) fn retained_count(&self) -> usize {
            self.retained.len()
        }
    }

    /// Resolve a relative path against the current directory, then retain the drive root and every ancestor. Does not open or create the leaf.
    #[allow(
        dead_code,
        reason = "Windows unit tests use the one-arg wrapper; create-only uses prepare_publication_path_with_terminals"
    )]
    pub(crate) fn prepare_publication_path(
        input: &str,
    ) -> Result<WindowsPublicationPath, PublicationPathError> {
        prepare_publication_path_with_terminals(input, &[])
    }

    pub(crate) fn prepare_publication_path_with_terminals(
        input: &str,
        extra_terminals: &[&str],
    ) -> Result<WindowsPublicationPath, PublicationPathError> {
        let spec = parse_publication_path(input).map_err(PublicationPathError::Parse)?;
        let spec = resolve_effective_spec(spec)?;
        let drive = match &spec {
            PublicationPathSpec::DriveAbsolute { drive, .. } => *drive,
            PublicationPathSpec::CurrentDirectoryRelative { .. } => {
                unreachable!("resolve_effective_spec always returns DriveAbsolute")
            }
        };
        let (anchor, anchor_identity) = open_anchor(drive)?;
        let guid = volume_guid_path(anchor.as_raw_handle())?;
        let ancestor_names = spec
            .ancestors()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let leaf = spec.leaf();
        let mut terminals = Vec::with_capacity(1 + extra_terminals.len());
        terminals.push(leaf);
        terminals.extend_from_slice(extra_terminals);
        check_terminal_names_budget(&guid, &ancestor_names, &terminals)
            .map_err(|_| PublicationPathError::PathTooLong)?;

        let mut retained: Vec<(OwnedHandle, WindowsFileIdentity)> =
            Vec::with_capacity(spec.ancestors().len() + 1);
        retained.push((anchor, anchor_identity));
        for name in spec.ancestors() {
            let parent = retained.last().expect("root just pushed").0.as_raw_handle();
            let (handle, identity) = open_or_create_ancestor(parent, name)?;
            retained.push((handle, identity));
        }

        Ok(WindowsPublicationPath {
            retained,
            diagnostic_path: PathBuf::from(input),
            leaf: OsString::from(spec.leaf()),
            move_spelling: OsString::from_wide(&join_guid_parent(&guid, &ancestor_names)),
        })
    }

    fn resolve_effective_spec(
        spec: PublicationPathSpec,
    ) -> Result<PublicationPathSpec, PublicationPathError> {
        let PublicationPathSpec::CurrentDirectoryRelative {
            components: caller_components,
        } = spec
        else {
            return Ok(spec);
        };
        let cwd = std::env::current_dir().map_err(|source| PublicationPathError::Io {
            operation: "resolve publication-path current directory",
            source,
        })?;
        let cwd_str = cwd.to_str().ok_or_else(|| PublicationPathError::Io {
            operation: "resolve publication-path current directory",
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "current directory is not valid UTF-8",
            ),
        })?;
        match parse_publication_path(cwd_str) {
            Ok(PublicationPathSpec::DriveAbsolute {
                drive,
                components: mut cwd_components,
            }) => {
                cwd_components.extend(caller_components);
                Ok(PublicationPathSpec::DriveAbsolute {
                    drive,
                    components: cwd_components,
                })
            }
            Ok(PublicationPathSpec::CurrentDirectoryRelative { .. }) => {
                unreachable!(
                    "GetCurrentDirectoryW never returns a bare-relative or already-refused string"
                )
            }
            Err(PublicationPathParseError::RootOnly) => {
                let drive = ascii_drive_letter(cwd_str).expect(
                    "parse_publication_path already matched a drive letter to produce RootOnly",
                );
                Ok(PublicationPathSpec::DriveAbsolute {
                    drive,
                    components: caller_components,
                })
            }
            Err(other) => Err(PublicationPathError::Parse(other)),
        }
    }

    /// Open the drive root as the retained chain's first handle.
    fn open_anchor(drive: u8) -> Result<(OwnedHandle, WindowsFileIdentity), PublicationPathError> {
        let wide = [u16::from(drive), u16::from(b':'), u16::from(b'\\'), 0];
        let handle = open_anchor_directory(&wide)?;
        let identity = validate_retained_directory(handle.as_raw_handle(), "<anchor>")?;
        Ok((handle, identity))
    }

    fn open_anchor_directory(wide: &[u16]) -> Result<OwnedHandle, PublicationPathError> {
        // SAFETY: `wide` is NUL-terminated and remains live for the synchronous CreateFileW call.
        #[allow(unsafe_code)]
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                DIRECTORY_ACCESS,
                ANCHOR_SHARE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(PublicationPathError::Io {
                operation: "open publication-path anchor",
                source: io::Error::last_os_error(),
            });
        }
        // SAFETY: CreateFileW returned one valid owned handle, converted exactly once.
        #[allow(unsafe_code)]
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    fn volume_guid_path(handle: RawHandle) -> Result<Vec<u16>, PublicationPathError> {
        let mut buffer = vec![0u16; GUID_PATH_INITIAL_UNITS];
        loop {
            // SAFETY: `buffer` is writable for `len` UTF-16 units and `handle` remains valid.
            #[allow(unsafe_code)]
            let written = unsafe {
                GetFinalPathNameByHandleW(
                    handle,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    VOLUME_NAME_GUID,
                )
            };
            if written == 0 {
                return Err(PublicationPathError::UnstableNamespace {
                    source: io::Error::last_os_error(),
                });
            }
            let written = written as usize;
            if written < buffer.len() {
                buffer.truncate(written);
                break;
            }
            let needed = written.max(buffer.len().saturating_add(1));
            if needed > MAX_EXTENDED_PATH_UTF16.saturating_add(1) {
                return Err(PublicationPathError::PathTooLong);
            }
            buffer.resize(needed, 0);
        }
        if !buffer.starts_with(&VOLUME_GUID_PREFIX) {
            return Err(PublicationPathError::UnstableNamespace {
                source: io::Error::other("anchor is not a Mount-Manager volume GUID path"),
            });
        }
        Ok(buffer)
    }

    fn open_or_create_ancestor(
        parent: RawHandle,
        name: &str,
    ) -> Result<(OwnedHandle, WindowsFileIdentity), PublicationPathError> {
        let os_name = OsStr::new(name);
        match nt_create_relative_deny_delete_sharing(
            parent,
            os_name,
            DIRECTORY_ACCESS,
            FILE_OPEN,
            DIRECTORY_OPTIONS,
        ) {
            Ok(handle) => {
                let identity = validate_retained_directory(handle.as_raw_handle(), name)?;
                Ok((handle, identity))
            }
            Err(source) if is_not_found(&source) => create_then_reopen_ancestor(parent, name),
            Err(source) if is_not_directory(&source) => Err(PublicationPathError::NotDirectory {
                component: name.to_owned(),
            }),
            Err(source) => Err(PublicationPathError::Io {
                operation: "open publication-path ancestor",
                source,
            }),
        }
    }

    fn create_then_reopen_ancestor(
        parent: RawHandle,
        name: &str,
    ) -> Result<(OwnedHandle, WindowsFileIdentity), PublicationPathError> {
        let os_name = OsStr::new(name);
        let created = match nt_create_relative_deny_delete_sharing(
            parent,
            os_name,
            DIRECTORY_ACCESS,
            FILE_CREATE,
            DIRECTORY_OPTIONS,
        ) {
            Ok(handle) => handle,
            Err(source) if is_already_exists(&source) => {
                return Err(PublicationPathError::AlreadyExists {
                    component: name.to_owned(),
                });
            }
            Err(source) => {
                return Err(PublicationPathError::Io {
                    operation: "create publication-path ancestor",
                    source,
                });
            }
        };
        let created_identity = validate_retained_directory(created.as_raw_handle(), name)?;
        let reopened = nt_create_relative_deny_delete_sharing(
            parent,
            os_name,
            DIRECTORY_ACCESS,
            FILE_OPEN,
            DIRECTORY_OPTIONS,
        )
        .map_err(|source| PublicationPathError::Io {
            operation: "reopen publication-path ancestor",
            source,
        })?;
        let reopened_identity = validate_retained_directory(reopened.as_raw_handle(), name)?;
        if created_identity != reopened_identity {
            return Err(PublicationPathError::IdentityMismatch {
                component: name.to_owned(),
            });
        }
        drop(created);
        Ok((reopened, reopened_identity))
    }

    fn open_move_spelling(spelling: &OsStr) -> Result<OwnedHandle, PublicationPathError> {
        let mut wide = spelling.encode_wide().collect::<Vec<_>>();
        if wide.contains(&0) {
            return Err(PublicationPathError::Io {
                operation: "open publication-path move spelling",
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "move spelling contains an interior NUL",
                ),
            });
        }
        wide.push(0);
        // SAFETY: `wide` is NUL-terminated and remains live for the synchronous CreateFileW call.
        #[allow(unsafe_code)]
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                DIRECTORY_ACCESS,
                PROBE_SHARE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(PublicationPathError::Io {
                operation: "open publication-path move spelling",
                source: io::Error::last_os_error(),
            });
        }
        // SAFETY: CreateFileW returned one valid owned handle, converted exactly once.
        #[allow(unsafe_code)]
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    fn validate_retained_directory(
        handle: RawHandle,
        component: &str,
    ) -> Result<WindowsFileIdentity, PublicationPathError> {
        let attributes = attribute_tag(handle)?;
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(PublicationPathError::ReparsePoint {
                component: component.to_owned(),
            });
        }
        if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(PublicationPathError::NotDirectory {
                component: component.to_owned(),
            });
        }
        file_identity(handle).map_err(|source| PublicationPathError::Io {
            operation: "query publication-path identity",
            source,
        })
    }

    fn attribute_tag(handle: RawHandle) -> Result<FILE_ATTRIBUTE_TAG_INFO, PublicationPathError> {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: `info` is writable for its exact buffer size and `handle` remains valid.
        #[allow(unsafe_code)]
        let result = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        (result != 0)
            .then_some(info)
            .ok_or_else(|| PublicationPathError::Io {
                operation: "query publication-path attributes",
                source: io::Error::last_os_error(),
            })
    }

    fn is_not_found(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
        )
    }

    fn is_already_exists(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_FILE_EXISTS as i32 || code == ERROR_ALREADY_EXISTS as i32
        )
    }

    fn is_not_directory(error: &io::Error) -> bool {
        error.raw_os_error() == Some(ERROR_DIRECTORY as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_EXTENDED_PATH_UTF16, PublicationPathParseError, PublicationPathSpec,
        check_move_spelling_budget, check_terminal_names_budget, join_guid_parent,
        parse_publication_path,
    };
    use crate::name_admission::NameAdmissionReason;

    #[test]
    fn parse_rejects_empty() {
        assert_eq!(
            parse_publication_path(""),
            Err(PublicationPathParseError::Empty)
        );
    }

    #[test]
    fn parse_rejects_root_only_backslash() {
        assert_eq!(
            parse_publication_path(r"C:\"),
            Err(PublicationPathParseError::RootOnly)
        );
    }

    #[test]
    fn parse_rejects_root_only_slash() {
        assert_eq!(
            parse_publication_path("C:/"),
            Err(PublicationPathParseError::RootOnly)
        );
    }

    #[test]
    fn parse_rejects_drive_relative_bare() {
        assert_eq!(
            parse_publication_path("C:"),
            Err(PublicationPathParseError::DriveRelative)
        );
    }

    #[test]
    fn parse_rejects_drive_relative_path() {
        assert_eq!(
            parse_publication_path("C:foo"),
            Err(PublicationPathParseError::DriveRelative)
        );
    }

    #[test]
    fn parse_rejects_drive_relative_nested() {
        assert_eq!(
            parse_publication_path(r"C:foo\bar"),
            Err(PublicationPathParseError::DriveRelative)
        );
    }

    #[test]
    fn parse_rejects_rooted_without_drive_backslash() {
        assert_eq!(
            parse_publication_path(r"\foo"),
            Err(PublicationPathParseError::RootedWithoutDrive)
        );
    }

    #[test]
    fn parse_rejects_rooted_without_drive_slash() {
        assert_eq!(
            parse_publication_path("/foo"),
            Err(PublicationPathParseError::RootedWithoutDrive)
        );
    }

    #[test]
    fn parse_rejects_rooted_without_drive_root() {
        assert_eq!(
            parse_publication_path("\\"),
            Err(PublicationPathParseError::RootedWithoutDrive)
        );
    }

    #[test]
    fn parse_rejects_unc() {
        assert_eq!(
            parse_publication_path(r"\\server\share\foo"),
            Err(PublicationPathParseError::UncOrDevicePrefix)
        );
    }

    #[test]
    fn parse_rejects_forward_unc() {
        assert_eq!(
            parse_publication_path("//server/share"),
            Err(PublicationPathParseError::UncOrDevicePrefix)
        );
    }

    #[test]
    fn parse_rejects_verbatim() {
        assert_eq!(
            parse_publication_path(r"\\?\C:\foo"),
            Err(PublicationPathParseError::UncOrDevicePrefix)
        );
    }

    #[test]
    fn parse_rejects_device() {
        assert_eq!(
            parse_publication_path(r"\\.\pipe\x"),
            Err(PublicationPathParseError::UncOrDevicePrefix)
        );
    }

    #[test]
    fn parse_rejects_dot() {
        assert_eq!(
            parse_publication_path("."),
            Err(PublicationPathParseError::DotComponent)
        );
    }

    #[test]
    fn parse_rejects_dotdot() {
        assert_eq!(
            parse_publication_path(".."),
            Err(PublicationPathParseError::DotComponent)
        );
    }

    #[test]
    fn parse_rejects_nested_dotdot() {
        assert_eq!(
            parse_publication_path(r"foo\.."),
            Err(PublicationPathParseError::DotComponent)
        );
    }

    #[test]
    fn parse_rejects_interior_nul() {
        assert_eq!(
            parse_publication_path("foo\0bar"),
            Err(PublicationPathParseError::InteriorNul)
        );
    }

    #[test]
    fn parse_rejects_control() {
        assert_eq!(
            parse_publication_path("a\nb"),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::Control
            ))
        );
    }

    #[test]
    fn parse_rejects_colon() {
        assert_eq!(
            parse_publication_path("foo:bar"),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::AlternateDataStream
            ))
        );
    }

    #[test]
    fn parse_rejects_forbidden() {
        assert_eq!(
            parse_publication_path("a<b"),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::ForbiddenCharacter
            ))
        );
    }

    #[test]
    fn parse_rejects_too_long() {
        assert_eq!(
            parse_publication_path(&"a".repeat(256)),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::TooLong
            ))
        );
    }

    #[test]
    fn parse_rejects_reserved_con() {
        assert_eq!(
            parse_publication_path("CON"),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::ReservedDevice
            ))
        );
    }

    #[test]
    fn parse_rejects_reserved_com1() {
        assert_eq!(
            parse_publication_path("com1"),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::ReservedDevice
            ))
        );
    }

    #[test]
    fn parse_rejects_trailing_dot() {
        assert_eq!(
            parse_publication_path("foo."),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::TrailingDotOrSpace
            ))
        );
    }

    #[test]
    fn parse_rejects_trailing_space() {
        assert_eq!(
            parse_publication_path("foo "),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::TrailingDotOrSpace
            ))
        );
    }

    #[test]
    fn parse_rejects_empty_component() {
        assert_eq!(
            parse_publication_path(r"foo\\bar"),
            Err(PublicationPathParseError::InvalidComponent(
                NameAdmissionReason::Empty
            ))
        );
    }

    #[test]
    fn parse_accepts_relative_leaf() {
        assert_eq!(
            parse_publication_path("leaf"),
            Ok(PublicationPathSpec::CurrentDirectoryRelative {
                components: vec!["leaf".to_owned()],
            })
        );
    }

    #[test]
    fn parse_accepts_relative_multi_backslash() {
        let spec = parse_publication_path(r"a\b\leaf").unwrap();
        assert_eq!(
            spec,
            PublicationPathSpec::CurrentDirectoryRelative {
                components: vec!["a".to_owned(), "b".to_owned(), "leaf".to_owned()],
            }
        );
        assert_eq!(spec.ancestors(), ["a", "b"]);
        assert_eq!(spec.leaf(), "leaf");
    }

    #[test]
    fn parse_accepts_relative_multi_slash() {
        assert_eq!(
            parse_publication_path("a/b/leaf"),
            Ok(PublicationPathSpec::CurrentDirectoryRelative {
                components: vec!["a".to_owned(), "b".to_owned(), "leaf".to_owned()],
            })
        );
    }

    #[test]
    fn parse_accepts_relative_mixed_separators() {
        assert_eq!(
            parse_publication_path(r"a\b/c"),
            Ok(PublicationPathSpec::CurrentDirectoryRelative {
                components: vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            })
        );
    }

    #[test]
    fn parse_accepts_drive_absolute_leaf() {
        assert_eq!(
            parse_publication_path(r"C:\leaf"),
            Ok(PublicationPathSpec::DriveAbsolute {
                drive: b'C',
                components: vec!["leaf".to_owned()],
            })
        );
    }

    #[test]
    fn parse_accepts_drive_absolute_slash() {
        assert_eq!(
            parse_publication_path("C:/leaf"),
            Ok(PublicationPathSpec::DriveAbsolute {
                drive: b'C',
                components: vec!["leaf".to_owned()],
            })
        );
    }

    #[test]
    fn parse_accepts_lowercase_drive() {
        assert_eq!(
            parse_publication_path(r"c:\a\b\leaf"),
            Ok(PublicationPathSpec::DriveAbsolute {
                drive: b'C',
                components: vec!["a".to_owned(), "b".to_owned(), "leaf".to_owned()],
            })
        );
    }

    #[test]
    fn parse_accepts_drive_absolute_multi() {
        let spec = parse_publication_path(r"C:\a\b\leaf").unwrap();
        assert_eq!(spec.ancestors(), ["a", "b"]);
        assert_eq!(spec.leaf(), "leaf");
    }

    #[test]
    fn join_preserves_trailing_slash_when_empty() {
        let prefix = vec![
            b'\\' as u16,
            b'\\' as u16,
            b'?' as u16,
            b'\\' as u16,
            b'V' as u16,
            b'\\' as u16,
        ];
        assert_eq!(join_guid_parent(&prefix, &[]), prefix);
    }

    #[test]
    fn join_strips_and_appends_ancestors() {
        let prefix = vec![b'X' as u16, b'\\' as u16];
        let joined = join_guid_parent(&prefix, &["a", "b"]);
        assert_eq!(
            joined,
            vec![
                b'X' as u16,
                b'\\' as u16,
                b'a' as u16,
                b'\\' as u16,
                b'b' as u16,
            ]
        );
    }

    #[test]
    fn budget_accepts_ordinary_prefix_and_components() {
        let prefix: Vec<u16> = "\\\\?\\Volume{00000000-0000-0000-0000-000000000000}\\"
            .encode_utf16()
            .collect();
        check_move_spelling_budget(&prefix, &["a", "b"]).unwrap();
        assert!(join_guid_parent(&prefix, &["a", "b"]).len() <= MAX_EXTENDED_PATH_UTF16);
    }

    #[test]
    fn budget_accepts_empty_ancestors_at_bound() {
        let prefix = vec![b'x' as u16; MAX_EXTENDED_PATH_UTF16];
        check_move_spelling_budget(&prefix, &[]).unwrap();
    }

    #[test]
    fn budget_rejects_empty_ancestors_over_bound() {
        let prefix = vec![b'x' as u16; MAX_EXTENDED_PATH_UTF16 + 1];
        assert!(check_move_spelling_budget(&prefix, &[]).is_err());
    }

    #[test]
    fn budget_rejects_joined_components_over_bound() {
        let prefix = vec![b'x' as u16; MAX_EXTENDED_PATH_UTF16 - 1];
        assert!(check_move_spelling_budget(&prefix, &["a"]).is_err());
    }

    #[test]
    fn budget_accepts_joined_components_at_bound() {
        let prefix = vec![b'x' as u16; MAX_EXTENDED_PATH_UTF16 - 2];
        check_move_spelling_budget(&prefix, &["a"]).unwrap();
        assert_eq!(
            join_guid_parent(&prefix, &["a"]).len(),
            MAX_EXTENDED_PATH_UTF16
        );
    }

    #[test]
    fn terminal_budget_accepts_destination_leaf_at_bound() {
        let prefix = vec![b'x' as u16; MAX_EXTENDED_PATH_UTF16 - 2];
        check_terminal_names_budget(&prefix, &[], &["a"]).unwrap();
    }

    #[test]
    fn terminal_budget_rejects_destination_leaf_over_bound() {
        let prefix = vec![b'x' as u16; MAX_EXTENDED_PATH_UTF16 - 1];
        assert!(check_terminal_names_budget(&prefix, &[], &["a"]).is_err());
    }

    #[test]
    fn terminal_budget_rejects_worst_case_stage_over_bound_when_destination_fits() {
        let prefix = vec![b'x' as u16; MAX_EXTENDED_PATH_UTF16 - 2];
        check_terminal_names_budget(&prefix, &[], &["a"]).unwrap();
        assert!(check_terminal_names_budget(&prefix, &[], &["a", "ab"]).is_err());
    }

    #[cfg(windows)]
    use super::windows_impl::{PublicationPathError, prepare_publication_path};
    #[cfg(windows)]
    use crate::test_support::TempDir;
    #[cfg(windows)]
    use std::ffi::OsStr;
    #[cfg(windows)]
    use std::fs;
    #[cfg(windows)]
    use std::path::{Path, PathBuf};
    #[cfg(windows)]
    use std::sync::{Mutex, MutexGuard};

    #[cfg(windows)]
    static CWD_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(windows)]
    fn cwd_test_lock() -> MutexGuard<'static, ()> {
        CWD_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(windows)]
    struct RestoreCwd(PathBuf);

    #[cfg(windows)]
    impl Drop for RestoreCwd {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[cfg(windows)]
    fn set_cwd(path: &Path) -> RestoreCwd {
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(path).unwrap();
        RestoreCwd(previous)
    }

    #[cfg(windows)]
    fn resolved_cwd_component_count() -> usize {
        let cwd = std::env::current_dir().unwrap();
        match parse_publication_path(cwd.to_str().unwrap()).unwrap() {
            PublicationPathSpec::DriveAbsolute { components, .. } => components.len(),
            PublicationPathSpec::CurrentDirectoryRelative { .. } => unreachable!(),
        }
    }

    #[cfg(windows)]
    fn assert_share_blocked(path: &Path) {
        let rename_error = fs::rename(path, path.with_extension("retired")).unwrap_err();
        assert!(
            rename_error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
                || rename_error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32),
            "rename of {} while live: {rename_error:?}",
            path.display()
        );
        let remove_error = fs::remove_dir(path).unwrap_err();
        assert!(
            remove_error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
                || remove_error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32),
            "remove_dir of {} while live: {remove_error:?}",
            path.display()
        );
    }

    #[cfg(windows)]
    fn assert_volume_guid_spelling(spelling: &OsStr) {
        let text = spelling.to_string_lossy();
        assert!(
            text.starts_with(r"\\?\Volume{"),
            "move spelling {text:?} is not a volume-GUID path"
        );
        assert!(
            !text.as_bytes().get(5).is_some_and(|byte| *byte == b':'),
            "move spelling {text:?} looks like a drive-letter path"
        );
    }

    #[cfg(windows)]
    #[test]
    fn prepare_volume_guid_move_spelling() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let _cwd = set_cwd(temporary.path());
        let prepared = prepare_publication_path(r"a\b\leaf").unwrap();
        assert_volume_guid_spelling(prepared.move_spelling());
        assert_eq!(prepared.leaf_name(), OsStr::new("leaf"));
        assert!(!temporary.path().join("a").join("b").join("leaf").exists());
        prepared.revalidate().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn prepare_cwd_direct_retains_anchor() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let _cwd = set_cwd(temporary.path());
        let prepared = prepare_publication_path("leaf").unwrap();
        assert_eq!(
            prepared.retained_count(),
            1 + resolved_cwd_component_count()
        );
        assert_eq!(prepared.leaf_name(), OsStr::new("leaf"));
        assert!(!temporary.path().join("leaf").exists());
        prepared.revalidate().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn prepare_multicomponent_retains_full_chain() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let _cwd = set_cwd(temporary.path());
        let prepared = prepare_publication_path(r"a\b\leaf").unwrap();
        assert_eq!(
            prepared.retained_count(),
            1 + resolved_cwd_component_count() + 2
        );
        assert!(temporary.path().join("a").is_dir());
        assert!(temporary.path().join("a").join("b").is_dir());
        assert!(!temporary.path().join("a").join("b").join("leaf").exists());
        prepared.revalidate().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn prepare_drive_root_leaf_not_created() {
        let temporary = TempDir::new();
        let drive = temporary
            .path()
            .to_str()
            .and_then(|text| text.chars().next())
            .expect("temp path has a drive letter");
        let leaf = format!("solstone-pubpath-{}", std::process::id());
        let input = format!(r"{drive}:\{leaf}");
        let prepared = prepare_publication_path(&input).unwrap();
        assert_eq!(prepared.retained_count(), 1);
        assert_eq!(prepared.leaf_name(), OsStr::new(&leaf));
        assert!(!Path::new(&input).exists());
        prepared.revalidate().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn prepare_refuses_file_at_ancestor() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let _cwd = set_cwd(temporary.path());
        fs::write(temporary.path().join("a"), b"not-a-directory").unwrap();
        let error = match prepare_publication_path(r"a\leaf") {
            Err(error) => error,
            Ok(_) => panic!("expected a file ancestor to be refused"),
        };
        assert!(matches!(
            error,
            PublicationPathError::NotDirectory { ref component } if component == "a"
        ));
        assert!(temporary.path().join("a").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn prepare_refuses_reparse_at_ancestor() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let _cwd = set_cwd(temporary.path());
        let target = temporary.path().join("target");
        fs::create_dir(&target).unwrap();
        std::os::windows::fs::symlink_dir(&target, temporary.path().join("a")).unwrap();
        let error = match prepare_publication_path(r"a\leaf") {
            Err(error) => error,
            Ok(_) => panic!("expected a reparse ancestor to be refused"),
        };
        assert!(matches!(
            error,
            PublicationPathError::ReparsePoint { ref component } if component == "a"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn prepare_leaves_prefix_on_later_refusal() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let _cwd = set_cwd(temporary.path());
        fs::create_dir(temporary.path().join("a")).unwrap();
        fs::write(temporary.path().join("a").join("b"), b"file").unwrap();
        let error = match prepare_publication_path(r"a\b\leaf") {
            Err(error) => error,
            Ok(_) => panic!("expected a file ancestor to be refused"),
        };
        assert!(matches!(
            error,
            PublicationPathError::NotDirectory { ref component } if component == "b"
        ));
        assert!(temporary.path().join("a").is_dir());
        assert!(temporary.path().join("a").join("b").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn revalidate_ok_on_real_chain() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let _cwd = set_cwd(temporary.path());
        let prepared = prepare_publication_path(r"a\b\leaf").unwrap();
        prepared.revalidate().unwrap();
        fs::create_dir(temporary.path().join("a").join("sibling")).unwrap();
        prepared.revalidate().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn prepare_beyond_max_path_still_succeeds() {
        let _cwd_serial = cwd_test_lock();
        let temporary = TempDir::new();
        let mut current = temporary.path().to_path_buf();
        while current.to_str().map(str::len).unwrap_or(0) < 220 {
            current = current.join("n".repeat(32));
            fs::create_dir(&current).unwrap();
        }
        let _cwd = set_cwd(&current);
        let ancestor = "a".repeat(40);
        let prepared = prepare_publication_path(&format!(r"{ancestor}\leaf")).unwrap();
        assert!(prepared.move_spelling().to_string_lossy().len() > 260);
        prepared.revalidate().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn share_mode_blocks_rename_and_delete_while_live() {
        let _cwd_serial = cwd_test_lock();
        let original = TempDir::new();
        let elsewhere = TempDir::new();
        let _restore = set_cwd(original.path());
        let prepared = prepare_publication_path(r"a\b\leaf").unwrap();
        let _moved = set_cwd(elsewhere.path());
        let a = original.path().join("a");
        let b = a.join("b");
        for path in [original.path(), a.as_path(), b.as_path()] {
            assert_share_blocked(path);
        }
        drop(prepared);
        fs::rename(&b, b.with_extension("retired")).unwrap();
        fs::rename(&a, a.with_extension("retired")).unwrap();
        let retired_root = original.path().parent().unwrap().join(format!(
            "retired-{}",
            original.path().file_name().unwrap().to_string_lossy()
        ));
        fs::rename(original.path(), &retired_root).unwrap();
        fs::remove_dir_all(&retired_root).unwrap();
    }
}
