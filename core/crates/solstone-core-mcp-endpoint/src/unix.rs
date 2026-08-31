// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::fd::AsFd;
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{AT_FDCWD, AtFlags, Flock, FlockArg, OFlag, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fchmod, fstat, fstatat, mkdirat};
use nix::unistd::{fsync, geteuid};
use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;
use solstone_core_journal_config::{
    McpEndpointCapability, mcp_endpoint_capability, read_journal_config_bound,
};
use solstone_core_journal_io::atomic::write_bytes_exclusive_bound;
use solstone_core_journal_io::errors::AtomicWriteError;
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_sol_link::committed::load_committed_identity_bound;

#[cfg(all(unix, any(test, feature = "test-hooks")))]
use crate::test_seam::{OwnerBootstrapPrimitive, checkpoint};
use crate::{McpEndpointBootstrapError, McpEndpointOwnerContext};

const ENDPOINT_DIRECTORY: &str = "mcp-endpoint";
const CREATE_LOCK: &str = ".create.lock";
const POP_KEY: &str = "pop.ed25519.pk8";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const MAX_POP_PKCS8_DER_BYTES: u64 = 512;
const POP_PKCS8_READ_LIMIT: u64 = MAX_POP_PKCS8_DER_BYTES + 1;

const DIRECTORY_OPEN_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
pub(crate) const FILE_OPEN_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
pub(crate) const LOCK_OPEN_FLAGS: OFlag = OFlag::O_RDWR
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

struct EndpointDirectory {
    file: File,
}

struct LoadedKey {
    identity: FileIdentity,
    bytes: Vec<u8>,
}

pub(super) fn bootstrap(
    journal_root: &Path,
) -> Result<Option<McpEndpointOwnerContext>, McpEndpointBootstrapError> {
    let root = JournalRoot::open(journal_root).map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let config =
        read_journal_config_bound(&root).map_err(|_| McpEndpointBootstrapError::ConfigRead)?;
    match mcp_endpoint_capability(&config).map_err(|_| McpEndpointBootstrapError::Capability)? {
        McpEndpointCapability::Disabled => return Ok(None),
        McpEndpointCapability::Enabled => {}
    }

    checkpoint(OwnerBootstrapPrimitive::CommittedIdentityLoad)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    revalidate_root_binding(&root).map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let committed =
        load_committed_identity_bound(&root).map_err(|_| McpEndpointBootstrapError::Endpoint)?;

    checkpoint(OwnerBootstrapPrimitive::RootRevalidateBeforeEndpoint)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    revalidate_root_binding(&root).map_err(|_| McpEndpointBootstrapError::Endpoint)?;

    checkpoint(OwnerBootstrapPrimitive::EffectiveUid)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let owner = geteuid().as_raw();
    let endpoint = create_or_open_endpoint_directory(&root, owner)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    ensure_named_handle_binding(
        &root,
        OsStr::new(ENDPOINT_DIRECTORY),
        &endpoint.file,
        OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeLock,
    )
    .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let lock = create_or_open_lock(&endpoint.file, owner)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    checkpoint(OwnerBootstrapPrimitive::LockAcquire)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let lock = Flock::lock(lock, FlockArg::LockExclusive)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    validate_named_regular_binding(
        &endpoint.file,
        OsStr::new(CREATE_LOCK),
        &lock,
        owner,
        OwnerBootstrapPrimitive::LockBindingAfterAcquire,
    )
    .map_err(|_| McpEndpointBootstrapError::Endpoint)?;

    let loaded = load_or_generate_key(&endpoint.file, owner)
        .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    let keypair = finalize_key(
        &root,
        &endpoint.file,
        &lock,
        owner,
        loaded.identity,
        &loaded.bytes,
    )
    .map_err(|_| McpEndpointBootstrapError::Endpoint)?;
    Ok(Some(McpEndpointOwnerContext {
        _private: (),
        committed,
        keypair,
    }))
}

fn create_or_open_endpoint_directory(
    root: &JournalRoot,
    owner: u32,
) -> io::Result<EndpointDirectory> {
    let name = OsStr::new(ENDPOINT_DIRECTORY);
    let mut created = false;
    let before = match probe(root, name, OwnerBootstrapPrimitive::DirectoryNoFollowProbe)? {
        Some(stat) => stat,
        None => {
            checkpoint(OwnerBootstrapPrimitive::DirectoryCreate)?;
            match mkdirat(root, name, mode(DIRECTORY_MODE)) {
                Ok(()) => created = true,
                Err(Errno::EEXIST) => {}
                Err(error) => return Err(errno_error(error)),
            }
            probe(root, name, OwnerBootstrapPrimitive::DirectoryNoFollowProbe)?
                .ok_or_else(identity_changed)?
        }
    };
    if !is_directory(&before) || before.st_uid != owner {
        return Err(invalid_entry());
    }

    checkpoint(OwnerBootstrapPrimitive::DirectoryOpen)?;
    let fd = openat(root, name, DIRECTORY_OPEN_FLAGS, Mode::empty()).map_err(errno_error)?;
    let file = File::from(fd);
    checkpoint(OwnerBootstrapPrimitive::DirectoryFstat)?;
    let after = fstat(&file).map_err(errno_error)?;
    if identity(&before) != identity(&after) || !is_directory(&after) || after.st_uid != owner {
        return Err(identity_changed());
    }

    if created {
        checkpoint(OwnerBootstrapPrimitive::DirectoryFchmod)?;
        fchmod(&file, mode(DIRECTORY_MODE)).map_err(errno_error)?;
        checkpoint(OwnerBootstrapPrimitive::DirectoryFstat)?;
        let restat = fstat(&file).map_err(errno_error)?;
        if identity(&after) != identity(&restat)
            || !is_exact_directory(&restat, owner, DIRECTORY_MODE)
        {
            return Err(identity_changed());
        }
    } else if !is_exact_directory(&after, owner, DIRECTORY_MODE) {
        return Err(invalid_entry());
    }

    checkpoint(OwnerBootstrapPrimitive::RootRevalidateAndFsync)?;
    revalidate_root_binding(root)?;
    fsync(root).map_err(errno_error)?;

    Ok(EndpointDirectory { file })
}

fn create_or_open_lock(parent: &File, owner: u32) -> io::Result<File> {
    let name = OsStr::new(CREATE_LOCK);
    for _ in 0..2 {
        let before = probe(parent, name, OwnerBootstrapPrimitive::LockNoFollowProbe)?;
        if let Some(before) = before {
            if !is_exact_regular(&before, owner, FILE_MODE) {
                return Err(invalid_entry());
            }
            checkpoint(OwnerBootstrapPrimitive::LockOpen)?;
            let fd = openat(parent, name, LOCK_OPEN_FLAGS, Mode::empty()).map_err(errno_error)?;
            let file = File::from(fd);
            checkpoint(OwnerBootstrapPrimitive::LockFstat)?;
            let after = fstat(&file).map_err(errno_error)?;
            if identity(&before) != identity(&after) || !is_exact_regular(&after, owner, FILE_MODE)
            {
                return Err(identity_changed());
            }
            return Ok(file);
        }

        checkpoint(OwnerBootstrapPrimitive::LockCreate)?;
        let created = openat(
            parent,
            name,
            LOCK_OPEN_FLAGS | OFlag::O_CREAT | OFlag::O_EXCL,
            mode(FILE_MODE),
        );
        let fd = match created {
            Ok(fd) => fd,
            Err(Errno::EEXIST) => continue,
            Err(error) => return Err(errno_error(error)),
        };
        let file = File::from(fd);
        let named = probe(parent, name, OwnerBootstrapPrimitive::LockNoFollowProbe)?
            .ok_or_else(identity_changed)?;
        checkpoint(OwnerBootstrapPrimitive::LockFstat)?;
        let opened = fstat(&file).map_err(errno_error)?;
        if identity(&named) != identity(&opened) || !is_regular(&opened) || opened.st_uid != owner {
            return Err(identity_changed());
        }
        checkpoint(OwnerBootstrapPrimitive::LockFchmod)?;
        fchmod(&file, mode(FILE_MODE)).map_err(errno_error)?;
        checkpoint(OwnerBootstrapPrimitive::LockFstat)?;
        let restat = fstat(&file).map_err(errno_error)?;
        if identity(&opened) != identity(&restat) || !is_exact_regular(&restat, owner, FILE_MODE) {
            return Err(identity_changed());
        }
        return Ok(file);
    }
    Err(identity_changed())
}

fn load_or_generate_key(parent: &File, owner: u32) -> io::Result<LoadedKey> {
    match load_existing_key(parent, owner)? {
        Some(loaded) => Ok(loaded),
        None => generate_and_publish_key(parent, owner),
    }
}

fn generate_and_publish_key(parent: &File, owner: u32) -> io::Result<LoadedKey> {
    checkpoint(OwnerBootstrapPrimitive::KeyGenerate)?;
    let pkcs8 =
        Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| invalid_entry())?;
    checkpoint(OwnerBootstrapPrimitive::KeyPublish)?;
    match write_bytes_exclusive_bound(parent, OsStr::new(POP_KEY), pkcs8.as_ref(), FILE_MODE) {
        Ok(()) => load_existing_key(parent, owner)?.ok_or_else(identity_changed),
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists
                || source.raw_os_error() == Some(nix::libc::EEXIST) =>
        {
            Err(identity_changed())
        }
        Err(_) => Err(invalid_entry()),
    }
}

fn load_existing_key(parent: &File, owner: u32) -> io::Result<Option<LoadedKey>> {
    let name = OsStr::new(POP_KEY);
    let before = match probe(parent, name, OwnerBootstrapPrimitive::KeyPrecheckStat)? {
        Some(stat) => stat,
        None => return Ok(None),
    };
    validate_key_stat(&before, owner)?;

    checkpoint(OwnerBootstrapPrimitive::KeyOpen)?;
    let fd = openat(parent, name, FILE_OPEN_FLAGS, Mode::empty()).map_err(errno_error)?;
    let mut file = File::from(fd);
    checkpoint(OwnerBootstrapPrimitive::KeyFstat)?;
    let opened = fstat(&file).map_err(errno_error)?;
    if identity(&before) != identity(&opened) || !same_key_metadata(&before, &opened, owner) {
        return Err(identity_changed());
    }

    checkpoint(OwnerBootstrapPrimitive::KeyRead)?;
    let bytes = read_pkcs8_bounded(&mut file)?;
    checkpoint(OwnerBootstrapPrimitive::KeyFinalRestat)?;
    let restat = fstat(&file).map_err(errno_error)?;
    if !same_key_metadata(&opened, &restat, owner) {
        return Err(identity_changed());
    }
    checkpoint(OwnerBootstrapPrimitive::KeyDecode)?;
    Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| invalid_entry())?;
    Ok(Some(LoadedKey {
        identity: identity(&restat),
        bytes,
    }))
}

fn finalize_key(
    root: &JournalRoot,
    parent: &File,
    lock: &File,
    owner: u32,
    expected: FileIdentity,
    expected_bytes: &[u8],
) -> io::Result<Ed25519KeyPair> {
    checkpoint(OwnerBootstrapPrimitive::FinalKeyOpen)?;
    let fd =
        openat(parent, OsStr::new(POP_KEY), FILE_OPEN_FLAGS, Mode::empty()).map_err(errno_error)?;
    let mut file = File::from(fd);
    checkpoint(OwnerBootstrapPrimitive::FinalKeyRestat)?;
    let stat = fstat(&file).map_err(errno_error)?;
    if identity(&stat) != expected || !is_exact_regular(&stat, owner, FILE_MODE) {
        return Err(identity_changed());
    }
    if key_size(&stat)? > MAX_POP_PKCS8_DER_BYTES {
        return Err(invalid_entry());
    }
    validate_named_regular_binding(
        parent,
        OsStr::new(POP_KEY),
        &file,
        owner,
        OwnerBootstrapPrimitive::FinalKeyInitialNameBinding,
    )?;
    checkpoint(OwnerBootstrapPrimitive::FinalKeyContentCompare)?;
    let bytes = read_pkcs8_bounded(&mut file)?;
    if bytes != expected_bytes {
        return Err(identity_changed());
    }
    Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| invalid_entry())?;
    checkpoint(OwnerBootstrapPrimitive::FinalKeyFsync)?;
    fsync(&file).map_err(errno_error)?;
    checkpoint(OwnerBootstrapPrimitive::FinalDirectoryFsync)?;
    fsync(parent).map_err(errno_error)?;

    validate_named_regular_binding(
        parent,
        OsStr::new(CREATE_LOCK),
        lock,
        owner,
        OwnerBootstrapPrimitive::FinalLockBinding,
    )?;
    checkpoint(OwnerBootstrapPrimitive::FinalDirectoryRestat)?;
    let directory_stat = fstat(parent).map_err(errno_error)?;
    if !is_exact_directory(&directory_stat, owner, DIRECTORY_MODE) {
        return Err(identity_changed());
    }
    checkpoint(OwnerBootstrapPrimitive::FinalKeyAuthorityRestat)?;
    let final_stat = fstat(&file).map_err(errno_error)?;
    if identity(&final_stat) != expected
        || !is_exact_regular(&final_stat, owner, FILE_MODE)
        || key_size(&final_stat)? > MAX_POP_PKCS8_DER_BYTES
    {
        return Err(identity_changed());
    }
    validate_named_regular_binding(
        parent,
        OsStr::new(POP_KEY),
        &file,
        owner,
        OwnerBootstrapPrimitive::FinalKeyNameBinding,
    )?;
    checkpoint(OwnerBootstrapPrimitive::FinalRootRevalidate)?;
    revalidate_root_binding(root)?;
    validate_named_directory_binding(
        root,
        OsStr::new(ENDPOINT_DIRECTORY),
        parent,
        owner,
        OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeSuccess,
    )?;

    checkpoint(OwnerBootstrapPrimitive::FinalKeySeek)?;
    file.seek(SeekFrom::Start(0))?;
    checkpoint(OwnerBootstrapPrimitive::FinalKeyRead)?;
    let bytes = read_pkcs8_bounded(&mut file)?;
    if bytes != expected_bytes {
        return Err(identity_changed());
    }
    checkpoint(OwnerBootstrapPrimitive::FinalKeyDecode)?;
    Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| invalid_entry())
}

fn validate_named_regular_binding(
    parent: &impl AsFd,
    name: &OsStr,
    retained: &File,
    owner: u32,
    primitive: OwnerBootstrapPrimitive,
) -> io::Result<()> {
    let named = probe(parent, name, primitive)?.ok_or_else(identity_changed)?;
    let retained = fstat(retained).map_err(errno_error)?;
    if identity(&named) != identity(&retained)
        || !is_exact_regular(&named, owner, FILE_MODE)
        || !is_exact_regular(&retained, owner, FILE_MODE)
    {
        return Err(identity_changed());
    }
    Ok(())
}

fn validate_named_directory_binding(
    parent: &impl AsFd,
    name: &OsStr,
    retained: &File,
    owner: u32,
    primitive: OwnerBootstrapPrimitive,
) -> io::Result<()> {
    let named = probe(parent, name, primitive)?.ok_or_else(identity_changed)?;
    let retained = fstat(retained).map_err(errno_error)?;
    if identity(&named) != identity(&retained)
        || !is_exact_directory(&named, owner, DIRECTORY_MODE)
        || !is_exact_directory(&retained, owner, DIRECTORY_MODE)
    {
        return Err(identity_changed());
    }
    Ok(())
}

fn ensure_named_handle_binding(
    parent: &impl AsFd,
    name: &OsStr,
    retained: &File,
    primitive: OwnerBootstrapPrimitive,
) -> io::Result<()> {
    let named = probe(parent, name, primitive)?.ok_or_else(identity_changed)?;
    let retained = fstat(retained).map_err(errno_error)?;
    if identity(&named) != identity(&retained) {
        return Err(identity_changed());
    }
    Ok(())
}

fn revalidate_root_binding(root: &JournalRoot) -> io::Result<()> {
    root.revalidate().map_err(io::Error::other)?;
    let named = fstatat(
        AT_FDCWD,
        root.canonical_path(),
        AtFlags::AT_SYMLINK_NOFOLLOW,
    )
    .map_err(errno_error)?;
    let retained = fstat(root).map_err(errno_error)?;
    if !is_directory(&named) || identity(&named) != identity(&retained) {
        return Err(identity_changed());
    }
    Ok(())
}

fn probe(
    parent: &impl AsFd,
    name: &OsStr,
    primitive: OwnerBootstrapPrimitive,
) -> io::Result<Option<FileStat>> {
    checkpoint(primitive)?;
    match fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(stat)),
        Err(Errno::ENOENT) => Ok(None),
        Err(error) => Err(errno_error(error)),
    }
}

fn read_pkcs8_bounded(file: &mut File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(POP_PKCS8_READ_LIMIT).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).map_err(|_| invalid_entry())? > MAX_POP_PKCS8_DER_BYTES {
        return Err(invalid_entry());
    }
    Ok(bytes)
}

fn validate_key_stat(stat: &FileStat, owner: u32) -> io::Result<()> {
    if !is_exact_regular(stat, owner, FILE_MODE) || key_size(stat)? > MAX_POP_PKCS8_DER_BYTES {
        return Err(invalid_entry());
    }
    Ok(())
}

pub(crate) fn same_key_metadata(left: &FileStat, right: &FileStat, owner: u32) -> bool {
    identity(left) == identity(right)
        && is_exact_regular(left, owner, FILE_MODE)
        && is_exact_regular(right, owner, FILE_MODE)
        && key_size(left).ok() == key_size(right).ok()
        && key_size(right).is_ok_and(|size| size <= MAX_POP_PKCS8_DER_BYTES)
}

fn key_size(stat: &FileStat) -> io::Result<u64> {
    u64::try_from(stat.st_size).map_err(|_| invalid_entry())
}

#[allow(clippy::unnecessary_cast)]
pub(crate) fn identity(stat: &FileStat) -> FileIdentity {
    FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    }
}

pub(crate) fn is_directory(stat: &FileStat) -> bool {
    SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR
}

pub(crate) fn is_regular(stat: &FileStat) -> bool {
    SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG
}

pub(crate) fn is_exact_directory(stat: &FileStat, owner: u32, required_mode: u32) -> bool {
    is_directory(stat) && stat.st_uid == owner && permission_bits(stat) == mode(required_mode)
}

pub(crate) fn is_exact_regular(stat: &FileStat, owner: u32, required_mode: u32) -> bool {
    is_regular(stat) && stat.st_uid == owner && permission_bits(stat) == mode(required_mode)
}

fn permission_bits(stat: &FileStat) -> Mode {
    Mode::from_bits_truncate(stat.st_mode) & mode(0o777)
}

fn mode(bits: u32) -> Mode {
    Mode::from_bits_truncate(bits as nix::libc::mode_t)
}

fn errno_error(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

fn invalid_entry() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid endpoint entry")
}

fn identity_changed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "endpoint entry changed")
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
fn checkpoint(_primitive: OwnerBootstrapPrimitive) -> io::Result<()> {
    Ok(())
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
#[derive(Clone, Copy)]
enum OwnerBootstrapPrimitive {
    CommittedIdentityLoad,
    RootRevalidateBeforeEndpoint,
    EffectiveUid,
    DirectoryNoFollowProbe,
    DirectoryCreate,
    DirectoryOpen,
    DirectoryFchmod,
    DirectoryFstat,
    RootRevalidateAndFsync,
    DirectoryBindingCheckBeforeLock,
    LockNoFollowProbe,
    LockCreate,
    LockOpen,
    LockFchmod,
    LockFstat,
    LockAcquire,
    LockBindingAfterAcquire,
    KeyPrecheckStat,
    KeyOpen,
    KeyFstat,
    KeyRead,
    KeyFinalRestat,
    KeyDecode,
    KeyGenerate,
    KeyPublish,
    FinalKeyOpen,
    FinalKeyRestat,
    FinalKeyInitialNameBinding,
    FinalKeyContentCompare,
    FinalKeyFsync,
    FinalDirectoryFsync,
    FinalLockBinding,
    FinalDirectoryRestat,
    FinalKeyAuthorityRestat,
    FinalKeyNameBinding,
    FinalRootRevalidate,
    DirectoryBindingCheckBeforeSuccess,
    FinalKeySeek,
    FinalKeyRead,
    FinalKeyDecode,
}
