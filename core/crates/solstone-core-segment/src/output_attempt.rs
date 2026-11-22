// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared processing attempts and short segment publication exclusion.
//!
//! Acquire source claims first where applicable, then sorted output attempts,
//! then a segment commit. Inference runs while holding only output attempts.
//! Lock entries are persistent; removing one would split its lock namespace.

use solstone_core_cli_copy::MessageDetail;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    BoundParentLock, DirectoryIdentityWitness, ExistingParentLockError, JournalRoot, LockOptions,
    ObjectIdentity, acquire_existing_parent_lock_bound, open_existing_parent_lock_bound,
};
use solstone_core_processing_record::media::is_media_extension;

/// A coordination refusal. No publication should follow an error.
pub type Result<T> = std::result::Result<T, io::Error>;

fn refusal(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

#[derive(Debug)]
struct FixedRefusal(&'static str);
impl std::fmt::Display for FixedRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::ENGLISH,
            self.0,
            &[],
        ))
    }
}
impl std::error::Error for FixedRefusal {}

const CANCELLED_INPUT: &str = "journal.cli.transcribe.claim.input.cancelled";
fn fixed_refusal(id: &'static str) -> io::Error {
    io::Error::other(FixedRefusal(id))
}

pub fn input_verification_cancelled(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<FixedRefusal>())
        .is_some_and(|cause| cause.0 == CANCELLED_INPUT)
}

#[cfg(test)]
#[test]
fn only_the_hash_checkpoint_marks_an_input_verification_cancellation() {
    let cancelled = fixed_refusal(CANCELLED_INPUT);
    let unrelated = io::Error::other(cancelled.to_string());
    assert!(input_verification_cancelled(&cancelled));
    assert!(!input_verification_cancelled(&unrelated));
}

/// Preserve typed claim failures until the caller selects its display language.
pub fn claim_detail(error: &io::Error) -> MessageDetail {
    let Some(cause) = error.get_ref() else {
        return error.to_string().into();
    };
    if let Some(reason) = cause.downcast_ref::<FixedRefusal>() {
        return MessageDetail::message(reason.0, []);
    }
    if let Some(reason) = cause.downcast_ref::<solstone_core_journal_io::JournalRootError>() {
        use solstone_core_journal_io::JournalRootError as Root;
        return match reason {
            Root::Invalid { root, reason, .. } => root_reason(reason).context_with(
                "journal.cli.transcribe.claim.root.invalid",
                [("path", &root.display())],
            ),
            Root::Unsupported { root, reason, .. } => root_reason(reason).context_with(
                "journal.cli.transcribe.claim.root.unsupported",
                [("path", &root.display())],
            ),
            Root::Io {
                operation, source, ..
            } => MessageDetail::message(
                "journal.cli.transcribe.claim.root.io",
                [("operation", &(operation)), ("reason", &(source))],
            ),
            Root::Changed => {
                MessageDetail::message("journal.cli.transcribe.claim.root.changed", [])
            }
        };
    }
    if let Some(reason) = cause.downcast_ref::<ExistingParentLockError>() {
        use ExistingParentLockError as Lock;
        return match reason {
            Lock::InvalidLockPath { name } => MessageDetail::message(
                "journal.cli.transcribe.claim.lock.invalid",
                [("name", &(format!("{name:?}")))],
            ),
            Lock::MissingParent { parent } => MessageDetail::message(
                "journal.cli.transcribe.claim.lock.missing",
                [("path", &(parent.display()))],
            ),
            Lock::UnsafeParent { parent, kind } => MessageDetail::message(
                "journal.cli.transcribe.claim.lock.parent.unsafe",
                [("kind", &(kind)), ("path", &(parent.display()))],
            ),
            Lock::UnsafeLockEntry { path, kind } => MessageDetail::message(
                "journal.cli.transcribe.claim.lock.entry.unsafe",
                [("kind", &(kind)), ("path", &(path.display()))],
            ),
            Lock::WrongMode { path, observed } => MessageDetail::message(
                "journal.cli.transcribe.claim.lock.mode",
                [
                    ("mode", &(format!("{observed:o}"))),
                    ("path", &(path.display())),
                ],
            ),
            Lock::Io {
                operation,
                path,
                source,
            } => MessageDetail::message(
                "journal.cli.doctor.sync.io",
                [
                    ("operation", operation),
                    ("path", &path.display()),
                    ("reason", source),
                ],
            ),
            Lock::ParentChanged { parent } => MessageDetail::message(
                "journal.cli.transcribe.claim.lock.parent.changed",
                [("path", &(parent.display()))],
            ),
            Lock::NamespaceChanged { path } => MessageDetail::message(
                "journal.cli.transcribe.claim.lock.namespace",
                [("path", &(path.display()))],
            ),
            Lock::Timeout(timeout) => MessageDetail::message(
                "journal.cli.backup.config.lock.timeout",
                [
                    ("path", &timeout.path.display()),
                    ("seconds", &timeout.timeout.as_secs_f64()),
                ],
            ),
        };
    }
    error.to_string().into()
}

// These are typed root-refusal facts from journal-io, never input or helper text.
fn root_reason(reason: &'static str) -> MessageDetail {
    match reason {
        "journal root does not exist" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.absent", [])
        }
        "journal root is not a directory" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.directory", [])
        }
        "journal root must be absolute" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.absolute", [])
        }
        "canonical journal root has no final component" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.component", [])
        }
        "canonical journal root has a non-UTF-8 ancestor" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.ancestor", [])
        }
        "canonical journal root is not absolute" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.canonical.absolute", [])
        }
        technical => technical.into(),
    }
}

fn lock_refusal(error: ExistingParentLockError) -> io::Error {
    let kind = if matches!(error, ExistingParentLockError::Timeout(_)) {
        io::ErrorKind::WouldBlock
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(kind, error)
}

fn lock_name(path: &Path) -> Result<OsString> {
    let mut name = path
        .file_name()
        .ok_or_else(|| fixed_refusal("journal.cli.transcribe.claim.target.name"))?
        .to_os_string();
    name.push(".lock");
    Ok(name)
}

fn parent(path: &Path) -> Result<&Path> {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| fixed_refusal("journal.cli.transcribe.claim.parent.missing"))
}

fn admit(path: &Path) -> Result<JournalRoot> {
    JournalRoot::open(path).map_err(io::Error::other)
}

fn revalidate(root: &JournalRoot) -> Result<()> {
    root.revalidate_canonical_binding()
        .map_err(io::Error::other)
}

fn live(segment: &Path) -> Result<()> {
    if crate::owner_deleted(segment).map_err(|error| refusal(error.to_string()))? {
        return Err(fixed_refusal("journal.cli.segment.relocation.deleted"));
    }
    Ok(())
}

fn live_for(segment: &Path, locale: &str) -> Result<()> {
    if crate::owner_deleted(segment).map_err(|error| refusal(error.to_string()))? {
        return Err(refusal(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.segment.relocation.deleted",
            &[],
        )));
    }
    Ok(())
}

// Retain the admitted segment and its root ancestry, not just a pathname.
// Transplanting the same segment into a replacement journal is also a change.
struct DirectoryBinding {
    root: JournalRoot,
    ancestors: Vec<DirectoryIdentityWitness>,
}
impl DirectoryBinding {
    fn capture(path: &Path) -> Result<Self> {
        let root = admit(path)?;
        let ancestors = root
            .canonical_path()
            .ancestors()
            .skip(1)
            .map(|path| DirectoryIdentityWitness::open(path).map_err(io::Error::other))
            .collect::<Result<_>>()?;
        let binding = Self { root, ancestors };
        binding.revalidate()?;
        Ok(binding)
    }
    fn revalidate(&self) -> Result<()> {
        for ancestor in &self.ancestors {
            ancestor
                .revalidate_canonical_binding()
                .map_err(io::Error::other)?;
        }
        revalidate(&self.root)
    }
}

// Existing hold_lock entries may have the caller's ordinary file mode. Open
// the existing inode without changing permissions; only new entries need the
// bound creation primitive's private mode. Both paths refuse namespace swaps.
fn acquire_bound(
    root: &JournalRoot,
    name: &OsStr,
    options: LockOptions,
) -> Result<BoundParentLock> {
    if let Some(lock) =
        open_existing_parent_lock_bound(root, name, options.timeout, options.poll_interval)
            .map_err(lock_refusal)?
    {
        return Ok(lock);
    }
    acquire_existing_parent_lock_bound(root, name, options.timeout, options.poll_interval)
        .map_err(lock_refusal)
}

/// Names observed before acquiring a segment-wide collection of attempts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputDiscovery {
    names: BTreeSet<OsString>,
    outputs: BTreeSet<PathBuf>,
}

impl OutputDiscovery {
    /// Sorted, deduplicated output targets, including shared raw basename stems.
    pub fn outputs(&self) -> impl Iterator<Item = &PathBuf> {
        self.outputs.iter()
    }
}

/// Discover first; callers must never do this acquisition while holding segment exclusion.
pub fn discover_output_claims(segment: &Path) -> Result<OutputDiscovery> {
    let mut discovery = OutputDiscovery {
        names: BTreeSet::new(),
        outputs: BTreeSet::new(),
    };
    for entry in fs::read_dir(segment)? {
        let entry = entry?;
        let path = entry.path();
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension != "jsonl" && !is_media_extension(extension) {
            continue;
        }
        // Keep even occupied/non-plain candidates in the name set: removing or
        // replacing them must not silently shrink the discovered denominator.
        discovery.names.insert(entry.file_name());
        discovery.outputs.insert(path.with_extension("jsonl"));
    }
    Ok(discovery)
}

/// Sorted output attempts bound to the directories in which their locks were acquired.
pub struct OutputClaims {
    outputs: BTreeSet<PathBuf>,
    parents: BTreeMap<PathBuf, DirectoryBinding>,
    _locks: Vec<BoundParentLock>,
    discovered: Option<(PathBuf, OutputDiscovery)>,
    options: LockOptions,
}

impl OutputClaims {
    /// Claim fixed output targets. This does not claim unrelated media in the segment.
    pub fn acquire(
        outputs: impl IntoIterator<Item = PathBuf>,
        options: LockOptions,
    ) -> Result<Self> {
        let outputs: BTreeSet<_> = outputs.into_iter().collect();
        let mut parents = BTreeMap::new();
        let mut locks = Vec::new();
        for output in &outputs {
            let path = parent(output)?;
            if !parents.contains_key(path) {
                parents.insert(path.to_path_buf(), DirectoryBinding::capture(path)?);
            }
            let root = &parents[path];
            root.revalidate()?;
            locks.push(acquire_bound(&root.root, &lock_name(output)?, options)?);
            root.revalidate()?;
        }
        Ok(Self {
            outputs,
            parents,
            _locks: locks,
            discovered: None,
            options,
        })
    }

    /// Discover and acquire segment-wide attempts outside segment exclusion.
    /// A changed discovery set at commit is an explicit refusal; retry from here.
    pub fn discover(segment: &Path, options: LockOptions) -> Result<Self> {
        let admitted = DirectoryBinding::capture(segment)?;
        let discovery = discover_output_claims(segment)?;
        admitted.revalidate()?;
        let mut claims = Self::acquire(discovery.outputs.iter().cloned(), options)?;
        if let Some(root) = claims.parents.get(segment)
            && root.root.identity() != admitted.root.identity()
        {
            return Err(refusal("segment changed during output discovery"));
        }
        admitted.revalidate()?;
        claims.parents.insert(segment.to_path_buf(), admitted);
        claims.discovered = Some((segment.to_path_buf(), discovery));
        Ok(claims)
    }

    fn validate_parent(&self, segment: &Path) -> Result<()> {
        let root = self
            .parents
            .get(segment)
            .ok_or_else(|| fixed_refusal("journal.cli.transcribe.claim.segment.not.covered"))?;
        root.revalidate()
    }

    /// Exact retained identity of a covered parent, after binding validation.
    pub fn segment_identity(&self, segment: &Path) -> Result<ObjectIdentity> {
        self.validate_parent(segment)?;
        self.parents
            .get(segment)
            .map(|binding| binding.root.identity())
            .ok_or_else(|| fixed_refusal("journal.cli.transcribe.claim.segment.not.covered"))
    }

    fn revalidate_commit(&self, segment: &Path) -> Result<()> {
        for binding in self.parents.values() {
            binding.revalidate()?;
        }
        live(segment)?;
        if let Some((path, discovery)) = &self.discovered
            && (path != segment || *discovery != discover_output_claims(segment)?)
        {
            return Err(refusal(
                "segment output set changed; release claims and rediscover",
            ));
        }
        Ok(())
    }

    /// Hold the short segment exclusion through the caller's final publication.
    pub fn commit(&self, segment: &Path) -> Result<SegmentCommit<'_>> {
        self.validate_parent(segment)?;
        let segment_parent = admit(parent(segment)?)?;
        let lock = acquire_bound(&segment_parent, &lock_name(segment)?, self.options)?;
        revalidate(&segment_parent)?;
        self.revalidate_commit(segment)?;
        Ok(SegmentCommit {
            claims: self,
            parent: segment_parent,
            segment: segment.to_path_buf(),
            _lock: lock,
        })
    }

    /// Transfer to continuous live-name exclusion without retaining child locks.
    /// Independent readers can still cause a native directory operation to refuse.
    pub fn into_directory_mutation(mut self, segment: &Path) -> Result<DirectoryMutation> {
        let SegmentCommit {
            parent,
            segment,
            _lock,
            ..
        } = self.commit(segment)?;
        let binding = self.parents.remove(&segment).expect("validated parent");
        // The short lock is already held. Close child handles while keeping the
        // original directory and its ancestry alive, so identity cannot be reused.
        drop(self);
        Ok(DirectoryMutation {
            binding,
            parent,
            segment,
            _lock,
        })
    }
}

/// Original directory authority and live-name exclusion for a directory mutation.
pub struct DirectoryMutation {
    binding: DirectoryBinding,
    parent: JournalRoot,
    segment: PathBuf,
    _lock: BoundParentLock,
}

impl DirectoryMutation {
    /// Recheck original identity, ancestry and owner deletion without reopening children.
    pub fn revalidate(&self) -> Result<()> {
        self.binding.revalidate()?;
        revalidate(&self.parent)?;
        live(&self.segment)
    }
}

/// A publication boundary retaining its attempts and segment lock.
pub struct SegmentCommit<'a> {
    claims: &'a OutputClaims,
    parent: JournalRoot,
    segment: PathBuf,
    _lock: BoundParentLock,
}

impl SegmentCommit<'_> {
    /// Require output coverage from this exact commit's original claims.
    /// This only checks existing coverage and never acquires another lock.
    pub fn require_outputs(
        &self,
        claims: &OutputClaims,
        segment: &Path,
        outputs: impl IntoIterator<Item = PathBuf>,
    ) -> Result<()> {
        if !std::ptr::eq(self.claims, claims) || self.segment != segment {
            return Err(refusal("output coverage belongs to another segment commit"));
        }
        let mut any = false;
        for output in outputs {
            if output.parent() != Some(segment) || !claims.outputs.contains(&output) {
                return Err(refusal(
                    "requested output is not covered by this segment commit",
                ));
            }
            any = true;
        }
        if !any {
            return Err(refusal("requested output coverage is empty"));
        }
        self.revalidate()
    }

    /// Recheck the same claims, ancestry and discovery while exclusion stays held.
    /// This does not acquire another lock or cover additional output names.
    pub fn revalidate(&self) -> Result<()> {
        self.claims.validate_parent(&self.segment)?;
        revalidate(&self.parent)?;
        self.claims.revalidate_commit(&self.segment)
    }
}

/// Existing live-name exclusion for directory allocation or a directory pivot.
/// Existing content publication should use `OutputClaims::commit` instead.
pub struct SegmentMutation {
    _parent: JournalRoot,
    _lock: BoundParentLock,
}

impl SegmentMutation {
    /// Acquire after output attempts, when any exist. Does not create ancestors.
    pub fn acquire(segment: &Path, options: LockOptions) -> Result<Self> {
        Self::acquire_for(segment, options, solstone_core_cli_copy::ENGLISH)
    }

    pub(crate) fn acquire_for(segment: &Path, options: LockOptions, locale: &str) -> Result<Self> {
        let root = admit(parent(segment)?)?;
        let lock = acquire_bound(&root, &lock_name(segment)?, options)?;
        revalidate(&root)?;
        live_for(segment, locale)?;
        Ok(Self {
            _parent: root,
            _lock: lock,
        })
    }
}

#[derive(Eq, PartialEq)]
struct RawFacts {
    size: u64,
    digest: [u8; 32],
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(windows)]
    identity: solstone_core_journal_io::WindowsFileIdentity,
}

fn raw_facts_checked(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<(RawFacts, File)> {
    let named = fs::symlink_metadata(path)?;
    if !named.file_type().is_file() {
        return Err(fixed_refusal("journal.cli.transcribe.claim.input.not.file"));
    }
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        if named.dev() != metadata.dev() || named.ino() != metadata.ino() {
            return Err(fixed_refusal(
                "journal.cli.transcribe.claim.input.changed.open",
            ));
        }
        (metadata.dev(), metadata.ino())
    };
    #[cfg(windows)]
    let identity = solstone_core_journal_io::windows_file_identity(&file)?;
    let mut digest = Sha256::new();
    let mut size = 0;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err(fixed_refusal(CANCELLED_INPUT));
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        size += count as u64;
    }
    if size != metadata.len() || file.metadata()?.len() != size {
        return Err(fixed_refusal(
            "journal.cli.transcribe.claim.input.changed.read",
        ));
    }
    Ok((
        RawFacts {
            size,
            digest: digest.finalize().into(),
            identity,
        },
        file,
    ))
}

/// Exact raw input admitted under its output attempt before decoding/inference.
pub struct InputObservation {
    path: PathBuf,
    facts: RawFacts,
    // Keep the admitted inode alive through the attempt. Otherwise unlink and
    // recreate can reuse an inode number and make equal bytes look unchanged.
    _input: File,
}

impl InputObservation {
    /// Freeze current raw bytes and the claim's original parent binding.
    pub fn capture(claims: &OutputClaims, path: &Path) -> Result<Self> {
        Self::capture_checked(claims, path, &|| false)
    }

    /// Freeze the input while allowing the owner to interrupt long hashing.
    pub fn capture_checked(
        claims: &OutputClaims,
        path: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self> {
        if !claims.outputs.contains(&path.with_extension("jsonl")) {
            return Err(fixed_refusal(
                "journal.cli.transcribe.claim.input.no.attempt",
            ));
        }
        claims.validate_parent(parent(path)?)?;
        live(parent(path)?)?;
        let (facts, input) = raw_facts_checked(path, cancelled)?;
        claims.validate_parent(parent(path)?)?;
        Ok(Self {
            path: path.to_path_buf(),
            facts,
            _input: input,
        })
    }

    /// Byte count used by the unchanged processing-record vocabulary.
    pub fn input_size(&self) -> u64 {
        self.facts.size
    }

    /// SHA-256 of the exact source bytes admitted before processing.
    pub fn input_sha256(&self) -> [u8; 32] {
        self.facts.digest
    }

    /// Revalidate under segment exclusion and retain it through publication.
    pub fn commit<'a>(&self, claims: &'a OutputClaims) -> Result<SegmentCommit<'a>> {
        self.commit_checked(claims, &|| false)
    }

    /// Revalidate the exact bytes with cancellation before returning publication authority.
    pub fn commit_checked<'a>(
        &self,
        claims: &'a OutputClaims,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SegmentCommit<'a>> {
        if !claims.outputs.contains(&self.path.with_extension("jsonl")) {
            return Err(fixed_refusal(
                "journal.cli.transcribe.claim.input.no.attempt",
            ));
        }
        let commit = claims.commit(parent(&self.path)?)?;
        if raw_facts_checked(&self.path, cancelled)?.0 != self.facts {
            return Err(fixed_refusal(
                "journal.cli.transcribe.claim.input.changed.commit",
            ));
        }
        if cancelled() {
            return Err(fixed_refusal(CANCELLED_INPUT));
        }
        Ok(commit)
    }
}

/// Allocate a raw writer's segment before acquiring attempts. Never call during inference.
pub fn prepare_segment(segment: &Path) -> Result<()> {
    let directory = parent(segment)?;
    solstone_core_journal_io::ensure_directory(directory)
        .map_err(|error| refusal(error.to_string()))?;
    let root = admit(directory)?;
    let options = LockOptions::default();
    let _lock = acquire_bound(&root, &lock_name(segment)?, options)?;
    revalidate(&root)?;
    live(segment)?;
    solstone_core_journal_io::ensure_directory(segment)
        .map_err(|error| refusal(error.to_string()))?;
    admit(segment)?;
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn no_wait() -> LockOptions {
        LockOptions {
            timeout: std::time::Duration::ZERO,
            ..LockOptions::default()
        }
    }
    fn fixture() -> (TempDir, PathBuf, PathBuf) {
        let root = TempDir::new();
        let segment = root
            .path()
            .join("chronicle")
            .join("20260101")
            .join("work")
            .join("120000_60");
        fs::create_dir_all(&segment).unwrap();
        let raw = segment.join("audio.wav");
        fs::write(&raw, b"before").unwrap();
        (root, segment, raw)
    }

    #[test]
    fn directory_mutation_releases_child_claims_without_releasing_the_live_name() {
        let (root, segment, raw) = fixture();
        let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
        let mutation = claims.into_directory_mutation(&segment).unwrap();
        mutation.revalidate().unwrap();
        // A later attempt may open children, but cannot publish while this
        // mutation retains the short live-name lock.
        let contender = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        assert_eq!(
            contender.commit(&segment).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(contender);
        fs::rename(&segment, root.path().join("moved")).unwrap();
        assert!(mutation.revalidate().is_err());
        drop(mutation);
        assert!(!segment.exists());
        assert_eq!(
            fs::read(root.path().join("moved/audio.wav")).unwrap(),
            b"before"
        );
    }

    #[test]
    fn a_late_attempt_cannot_commit_after_directory_mutation_removes_its_parent() {
        let (_root, segment, raw) = fixture();
        fs::remove_file(raw).unwrap();
        let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
        let mutation = claims.into_directory_mutation(&segment).unwrap();
        let contender = OutputClaims::discover(&segment, no_wait()).unwrap();
        assert_eq!(
            contender.commit(&segment).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        mutation.revalidate().unwrap();
        fs::remove_dir(&segment).unwrap();
        drop(mutation);
        assert!(contender.commit(&segment).is_err());
        assert!(!segment.exists());
    }

    #[test]
    fn existing_writer_sidecars_keep_their_inode_and_mode() {
        let (_root, segment, raw) = fixture();
        let output = raw.with_extension("jsonl");
        // hold_lock is the preexisting writer/retention namespace.
        drop(
            solstone_core_journal_io::hold_lock(
                &output,
                LockOptions {
                    mode: Some(0o644),
                    ..no_wait()
                },
            )
            .unwrap(),
        );
        drop(
            solstone_core_journal_io::hold_lock(
                &segment,
                LockOptions {
                    mode: Some(0o644),
                    ..no_wait()
                },
            )
            .unwrap(),
        );
        #[cfg(unix)]
        let before = {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(output.with_extension("jsonl.lock")).unwrap();
            (metadata.ino(), metadata.mode())
        };
        let claims = OutputClaims::acquire([output.clone()], no_wait()).unwrap();
        let input = InputObservation::capture(&claims, &raw).unwrap();
        let _commit = input.commit(&claims).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(output.with_extension("jsonl.lock")).unwrap();
            assert_eq!((metadata.ino(), metadata.mode()), before);
        }
    }

    #[test]
    fn input_hashing_can_stop_before_capture_or_publication() {
        let (_root, _segment, raw) = fixture();
        fs::write(&raw, vec![7u8; 256 * 1024]).unwrap();
        let claims = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let polls = std::cell::Cell::new(0);
        let stop = || {
            polls.set(polls.get() + 1);
            polls.get() >= 3
        };
        assert!(InputObservation::capture_checked(&claims, &raw, &stop).is_err());
        assert_eq!(polls.get(), 3);
        let input = InputObservation::capture(&claims, &raw).unwrap();
        polls.set(0);
        assert!(input.commit_checked(&claims, &stop).is_err());
        assert_eq!(polls.get(), 3);
        assert!(!raw.with_extension("jsonl").exists());
        assert!(input.commit(&claims).is_ok());
    }

    #[test]
    fn unchanged_input_publishes_and_same_size_rewrite_refuses() {
        let (_root, segment, raw) = fixture();
        let claims = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let input = InputObservation::capture(&claims, &raw).unwrap();
        {
            let _commit = input.commit(&claims).unwrap();
            fs::write(raw.with_extension("jsonl"), b"proof").unwrap();
        }
        fs::write(&raw, b"after!").unwrap();
        assert!(input.commit(&claims).is_err());
        assert_eq!(fs::read(segment.join("audio.jsonl")).unwrap(), b"proof");
    }

    #[test]
    fn identical_bytes_at_a_recreated_raw_name_are_a_different_input() {
        let (_root, _segment, raw) = fixture();
        let claims = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let input = InputObservation::capture(&claims, &raw).unwrap();
        fs::remove_file(&raw).unwrap();
        fs::write(&raw, b"before").unwrap();
        assert!(input.commit(&claims).is_err());
    }

    #[test]
    fn deletion_and_replaced_parent_refuse() {
        let (_root, segment, raw) = fixture();
        let claims = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let input = InputObservation::capture(&claims, &raw).unwrap();
        fs::write(segment.join("tombstone.json"), b"{}").unwrap();
        assert!(input.commit(&claims).is_err());
        // Unix permits this forged ancestry while descendant claim handles remain open.
        #[cfg(unix)]
        {
            fs::remove_file(segment.join("tombstone.json")).unwrap();
            fs::rename(&segment, _root.path().join("moved")).unwrap();
            fs::create_dir(&segment).unwrap();
            fs::write(&raw, b"before").unwrap();
            assert!(input.commit(&claims).is_err());
            assert!(!segment.join("audio.jsonl").exists());
        }
    }

    // Unix permits renaming this subtree while descendant claim handles remain open.
    #[cfg(unix)]
    #[test]
    fn reused_path_cannot_transfer_a_claim_to_another_directory() {
        let (root, segment, raw) = fixture();
        let claims = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        fs::rename(&segment, root.path().join("moved")).unwrap();
        fs::create_dir(&segment).unwrap();
        fs::write(&raw, b"before").unwrap();
        let _replacement = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        assert!(InputObservation::capture(&claims, &raw).is_err());
        assert!(claims.commit(&segment).is_err());
    }

    // Unix permits transplanting ancestry while descendant claim handles remain open.
    #[cfg(unix)]
    #[test]
    fn transplanted_segment_cannot_transfer_to_a_replacement_journal() {
        let (root, segment, raw) = fixture();
        let claims = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let chronicle = root.path().join("chronicle");
        fs::rename(&chronicle, root.path().join("old-chronicle")).unwrap();
        fs::create_dir_all(segment.parent().unwrap()).unwrap();
        fs::rename(
            root.path()
                .join("old-chronicle")
                .join("20260101")
                .join("work")
                .join("120000_60"),
            &segment,
        )
        .unwrap();
        assert!(InputObservation::capture(&claims, &raw).is_err());
    }

    #[test]
    fn discovery_rescans_raw_names_even_when_the_output_set_is_unchanged() {
        let (_root, segment, raw) = fixture();
        let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
        assert!(claims.commit(&segment).is_ok());
        fs::write(raw.with_extension("flac"), b"another input").unwrap();
        assert!(claims.commit(&segment).is_err());
        drop(claims);
        let fixed = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let input = InputObservation::capture(&fixed, &raw).unwrap();
        fs::write(segment.join("screen.mp4"), b"unrelated").unwrap();
        assert!(input.commit(&fixed).is_ok());
    }

    #[test]
    fn held_output_coverage_requires_original_claims_and_every_requested_output() {
        let (_root, segment, raw) = fixture();
        let audio = raw.with_extension("jsonl");
        let screen = segment.join("screen.jsonl");
        let claims = OutputClaims::acquire([audio.clone(), screen.clone()], no_wait()).unwrap();
        let commit = claims.commit(&segment).unwrap();
        assert!(
            commit
                .require_outputs(&claims, &segment, [audio.clone(), screen])
                .is_ok()
        );
        assert!(
            commit
                .require_outputs(
                    &claims,
                    &segment,
                    [audio.clone(), segment.join("unheld.jsonl")]
                )
                .is_err()
        );
        assert!(
            commit
                .require_outputs(
                    &claims,
                    &segment,
                    [segment.join("nested").join("wrong.jsonl")]
                )
                .is_err()
        );
        assert!(commit.require_outputs(&claims, &segment, []).is_err());
        let other = OutputClaims::acquire([segment.join("other.jsonl")], no_wait()).unwrap();
        assert!(
            commit
                .require_outputs(&other, &segment, [audio.clone()])
                .is_err()
        );
        assert!(commit.require_outputs(&claims, &segment, [audio]).is_ok());
    }

    #[test]
    fn held_output_coverage_refuses_other_segment_and_late_tombstone() {
        let (_root, segment, raw) = fixture();
        let output = raw.with_extension("jsonl");
        let claims = OutputClaims::acquire([output.clone()], no_wait()).unwrap();
        let commit = claims.commit(&segment).unwrap();
        assert!(
            commit
                .require_outputs(&claims, &segment, [output.clone()])
                .is_ok()
        );
        assert!(
            commit
                .require_outputs(&claims, &segment.join("another"), [output.clone()])
                .is_err()
        );
        fs::write(segment.join("tombstone.json"), b"{}").unwrap();
        assert!(commit.require_outputs(&claims, &segment, [output]).is_err());
        assert_eq!(fs::read(raw).unwrap(), b"before");
    }

    #[test]
    fn held_commit_exposes_exact_identity_and_keeps_exclusion() {
        let (_root, segment, raw) = fixture();
        let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
        assert_eq!(
            claims.segment_identity(&segment).unwrap(),
            JournalRoot::open(&segment).unwrap().identity()
        );
        assert!(claims.segment_identity(&segment.join("uncovered")).is_err());
        let commit = claims.commit(&segment).unwrap();
        commit.revalidate().unwrap();
        assert!(solstone_core_journal_io::hold_lock(&segment, no_wait()).is_err());
        assert!(OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).is_err());
        commit.revalidate().unwrap();
    }

    #[test]
    fn typed_lock_timeouts_remain_distinct_from_unsafe_entries() {
        let (_root, segment, raw) = fixture();
        let output = raw.with_extension("jsonl");
        let holder = OutputClaims::acquire([output.clone()], no_wait()).unwrap();
        let error = OutputClaims::discover(&segment, no_wait()).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(matches!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<ExistingParentLockError>(),
            Some(ExistingParentLockError::Timeout(_))
        ));
        drop(holder);
        let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
        let segment_holder = solstone_core_journal_io::hold_lock(&segment, no_wait()).unwrap();
        let error = claims.commit(&segment).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(matches!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<ExistingParentLockError>(),
            Some(ExistingParentLockError::Timeout(_))
        ));
        drop(segment_holder);
        drop(claims.commit(&segment).unwrap());
        drop(claims);
        let lock = raw.with_extension("jsonl.lock");
        fs::remove_file(&lock).unwrap();
        fs::create_dir(&lock).unwrap();
        let error = OutputClaims::acquire([output], no_wait()).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(matches!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<ExistingParentLockError>(),
            Some(ExistingParentLockError::UnsafeLockEntry { .. })
        ));
    }

    #[test]
    fn held_discovery_refuses_new_names_while_fixed_claims_keep_their_scope() {
        let (_root, segment, raw) = fixture();
        let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
        let commit = claims.commit(&segment).unwrap();
        fs::write(raw.with_extension("flac"), b"same output stem").unwrap();
        assert!(commit.revalidate().is_err());
        drop(commit);
        drop(claims);
        let fixed = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let commit = fixed.commit(&segment).unwrap();
        fs::write(segment.join("unrelated.mp4"), b"new output").unwrap();
        commit.revalidate().unwrap();
    }

    #[test]
    fn held_commit_refuses_tombstone_replacement_and_transplanted_ancestry() {
        // Only Unix permits the rename forgeries; tombstone changes apply everywhere.
        let changes = if cfg!(unix) { 3 } else { 1 };
        for change in 0..changes {
            let (root, segment, _raw) = fixture();
            let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
            let commit = claims.commit(&segment).unwrap();
            match change {
                0 => fs::write(segment.join("tombstone.json"), b"{}").unwrap(),
                1 => {
                    fs::rename(&segment, root.path().join("moved")).unwrap();
                    fs::create_dir(&segment).unwrap();
                }
                _ => {
                    fs::rename(root.path().join("chronicle"), root.path().join("old")).unwrap();
                    fs::create_dir_all(segment.parent().unwrap()).unwrap();
                    fs::rename(
                        root.path()
                            .join("old")
                            .join("20260101")
                            .join("work")
                            .join("120000_60"),
                        &segment,
                    )
                    .unwrap();
                }
            }
            assert!(commit.revalidate().is_err());
            if change != 0 {
                assert!(claims.segment_identity(&segment).is_err());
            }
        }
    }

    #[test]
    fn empty_discovery_and_same_retained_ancestry_restoration_stay_valid() {
        let root = TempDir::new();
        let segment = root.path().join("parent").join("segment");
        fs::create_dir_all(&segment).unwrap();
        let claims = OutputClaims::discover(&segment, no_wait()).unwrap();
        let commit = claims.commit(&segment).unwrap();
        commit.revalidate().unwrap();
        // Only Unix permits moving and restoring an ancestor with retained children.
        #[cfg(unix)]
        {
            let parent = root.path().join("parent");
            let moved = root.path().join("moved");
            fs::rename(&parent, &moved).unwrap();
            assert!(commit.revalidate().is_err());
            fs::rename(&moved, &parent).unwrap();
            commit.revalidate().unwrap();
        }
    }

    #[test]
    fn shared_stems_deduplicate_and_reversed_callers_acquire_sorted() {
        let (_root, segment, raw) = fixture();
        let claims = OutputClaims::acquire(
            [
                raw.with_extension("jsonl"),
                raw.with_extension("flac").with_extension("jsonl"),
            ],
            no_wait(),
        )
        .unwrap();
        assert_eq!(claims.outputs.len(), 1);
        assert!(
            solstone_core_journal_io::hold_lock(raw.with_extension("jsonl"), no_wait()).is_err()
        );
        drop(claims);
        let _low = OutputClaims::acquire([segment.join("a.jsonl")], no_wait()).unwrap();
        assert!(
            OutputClaims::acquire(
                [segment.join("z.jsonl"), segment.join("a.jsonl")],
                no_wait()
            )
            .is_err()
        );
        assert!(!segment.join("z.jsonl.lock").exists());
    }

    #[test]
    fn final_commit_excludes_the_actual_relocation_pivot() {
        let (root, segment, raw) = fixture();
        let destination = segment.with_file_name("130000_60");
        let end = |key: &str| crate::RelocationEnd {
            day: "20260101".into(),
            segment: key.into(),
            disk_rel: format!("chronicle/20260101/work/{key}"),
            parent_rel: "chronicle/20260101/work".into(),
            path: crate::segment_dir::SegmentDir::resolve(root.path(), "20260101", key, "work")
                .unwrap()
                .path()
                .to_path_buf(),
        };
        let source = end("120000_60");
        let target = end("130000_60");
        let relocation = crate::Relocation {
            journal: root.path(),
            source: &source,
            destination: &target,
            successor: None,
            stream: "work",
            sequence: 1,
            locks: no_wait(),
        };
        let claims = OutputClaims::acquire([raw.with_extension("jsonl")], no_wait()).unwrap();
        let input = InputObservation::capture(&claims, &raw).unwrap();
        let commit = input.commit(&claims).unwrap();
        let held = SegmentMutation::acquire(&source.path, no_wait())
            .err()
            .unwrap();
        assert_eq!(held.kind(), io::ErrorKind::WouldBlock);
        let crate::RelocationRefusal::Failed(refused) =
            crate::relocate_segment(&relocation).unwrap_err()
        else {
            panic!("held commit must refuse at segment exclusion");
        };
        assert_eq!(refused.to_string(), held.to_string());
        fs::write(raw.with_extension("jsonl"), b"current proof").unwrap();
        drop(commit);
        for end in [&source, &target] {
            drop(SegmentMutation::acquire(&end.path, no_wait()).unwrap());
        }
        #[cfg(windows)]
        {
            // Windows keeps the directory in place while its child handles remain open.
            let crate::RelocationRefusal::Failed(refused) =
                crate::relocate_segment(&relocation).unwrap_err()
            else {
                panic!("open child handles must refuse the directory pivot");
            };
            assert_ne!(refused.to_string(), held.to_string());
            assert_eq!(fs::read(&raw).unwrap(), b"before");
            assert_eq!(
                fs::read(raw.with_extension("jsonl")).unwrap(),
                b"current proof"
            );
            assert!(!destination.exists());
            assert!(input.commit(&claims).is_ok());
            drop(input);
            drop(claims);
        }
        crate::relocate_segment(&relocation).unwrap();
        assert_eq!(
            fs::read(destination.join("audio.jsonl")).unwrap(),
            b"current proof"
        );
        #[cfg(not(windows))]
        assert!(input.commit(&claims).is_err());
        assert!(!segment.exists());
    }
}
