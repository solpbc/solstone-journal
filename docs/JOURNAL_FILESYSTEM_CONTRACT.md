# Journal filesystem contract

This is the shared vocabulary for a journal root, its identity, entry kinds, and
refusals. It is not a generic VFS. `solstone-core-journal-io` supports Unix and
Windows journal-root admission; `solstone-core-journal-archive` supports
source-only traversal on both platforms. Archive encoding and publication remain
Unix-only.

## Root, identity, kind, refusal

`solstone-core-journal-io` owns:

- **`JournalRoot`:** one admitted journal directory, retained by descriptor or
  handle.
- **`ObjectIdentity`:** opaque platform identity: Unix `(device, inode)` or
  Windows `(volume serial, 128-bit file ID)`. No public constructor, no
  accessors for the raw identity, no serde.
- **`JournalEntryKind`:** exhaustive no-follow kind: `RegularFile`,
  `Directory`, `Symlink`, `Fifo`, `Socket`, `CharacterDevice`, `BlockDevice`,
  `Other`.
- **`JournalRootError`:** `Invalid`, `Unsupported`, `Io`, `Changed`.

`JournalRoot` is not `Clone`: a cloned descriptor would be a second capability,
and a cloned path would be reacquisition. It is not serializable.

## Retained authority and no-follow

### Unix retained handle and no-follow

Admit once. The retained directory descriptor is source authority. Do not
reopen the root by path.

The canonical walk uses `O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW`. The
requested-root open omits `O_NOFOLLOW` so a symlink root is allowed. Descendant
opens in archive keep `O_NOFOLLOW` (regular files also `O_NONBLOCK`).

Revalidate the admitted object (`fstat` of the retained descriptor against the
frozen identity, and confirm it is still a directory). Do not walk the stored
canonical path to reacquire.

### Windows retained handle and gate-1 admission

Windows gate 1 admits only a journal root and its portable final name. Its
authoritative requested-root open uses `CreateFileW` with
`FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE` and
`FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT`. Admission also
opens a separate namespace-watch handle with the same access plus
`FILE_FLAG_OVERLAPPED`; it must independently pass directory/reparse checks and
match the authoritative handle's frozen identity. A reparse point at the root
or any inspected ancestor is refused; unlike Unix, this gate does not admit a
leaf symlink or junction.

Admission first captures attributes and `FileIdInfo` on the authoritative
listing handle, then admits the separate watch handle against that same frozen
identity. An independent absolute-path walk then opens every ancestor and the
target again, rejecting reparses and comparing the final target identity. The
retained handles are the only identity authorities. `revalidate()` reads the
listing handle only and never reopens a path.

Filesystem admission is a strict `NTFS`/`ReFS` allow-list, and volume admission
is a strict fixed-drive allow-list; each wildcard refuses rather than admits.
`NTFS` roots undergo Cloud Files classification; `ReFS` roots are admitted
without a `CfGetSyncRootInfoByPath` call. For `NTFS`, a registered sync root
refuses; HRESULTs `ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` (390),
`ERROR_NOT_A_CLOUD_FILE` (376), and `ERROR_INVALID_FUNCTION` (1)
admit; every other HRESULT refuses as unverifiable. `GetDriveTypeW` and
`CfGetSyncRootInfoByPath` are classification-only queries against the
already-validated path spelling, never identity reacquisition; identity comes
exclusively from `FileIdInfo` on the retained handle.

> **Known Windows gate-1 limitation:** the Win32 surface used here has no
> descriptor-relative open equivalent to Unix's `openat` + `O_NOFOLLOW` chain,
> so the ancestor verification uses separate absolute-path opens. The final
> target identity recheck catches final-target replacement, but cannot prove an
> intermediate ancestor was not transiently swapped and restored between those
> independent opens. This is materially weaker than Unix's descriptor-relative
> walk and is a known gate-1 limitation, not equivalent-strength authority.

### Windows retained-handle source operations

Windows source inventory and checked reads retain two independently admitted
root handles with one frozen identity: a synchronous listing/relative-open
handle and an overlapped namespace-watch handle. Admission checks each handle
as a non-reparse directory and refuses unless their `FileIdInfo` identities
match. Each witnessed operation first revalidates the listing handle (including
NTFS Cloud Files classification by retained handle). Root listing and the
checked-read relative-open parent seed borrow that handle; the watcher borrows
only the separately admitted watch handle and arms `ReadDirectoryChangesW` with
`bWatchSubtree=TRUE` and file- and directory-name notifications. This split
keeps a restartable enumeration cursor isolated from the asynchronous watch.
The listing handle is revalidated again before the result stands.

The watch is runtime-probed for each admitted NTFS or ReFS root. A completed
watch, `ERROR_NOTIFY_ENUM_DIR`, a zero-byte synchronous completion, or
inability to arm/check/cancel the watch refuses the whole operation; an
unsupported watch makes the capability `Unsupported` for that filesystem.
There is no pre/post pathname listing fallback. The root's own identity
revalidation remains necessary because directory change notifications do not
report every change to the watched directory object itself.

Recursive Windows inventory uses parent-relative child handles through the
handle-relative `NtCreateFile` route and `FileIdExtdDirectoryInfo`; every child
is checked as a non-reparse directory or regular file and matched to its
volume-serial plus 128-bit file identity before acceptance. To list a
descendant directory, inventory opens the same child name again relative to the
same verified parent with listing access, then rechecks that second handle's
identity against the first verification handle before recursing. It never
reopens an already-open handle or uses a pathname fallback. A checked file read
verifies every directory in its frozen route before reading, then rechecks the
leaf metadata, root, and witness before returning any bytes. Windows retains
the Gate-1 ancestor-swap and Cloud-Files-after-ancestor-rename limitation
above; this source layer does not claim to strengthen it.

### Windows retained-handle writer locks and leases

Windows writer coordination uses nonblocking whole-file advisory byte-range
locks through `LockFileEx` and `UnlockFileEx`. Lock and lease opens use broad
read, write, and delete sharing so a second process can open the same sidecar
and let the kernel report advisory contention rather than failing at open time.
The lock range begins at zero and spans `0xFFFF_FFFF` low and high length words.

Windows sidecar and persistent-lock opens request the reparse point itself and
refuse it after a retained-handle attribute query; this is the Windows analogue
of Unix `O_NOFOLLOW`. Persistent lock entries are revalidated after acquisition
with `LockEntryIdentity` (volume serial plus a 128-bit file ID), matching the
identity rule used by `ObjectIdentity`. Unix's mode-`0600` enforcement has no
Windows ACL equivalent in this backend and is therefore not claimed.

`InventoryBudget` bounds complete source operations: total observed entries
before portable policy filtering, recursive depth (admitted root is zero), one
portable slash-joined archive member's UTF-8 length, native relative UTF-16
path length, and cumulative bytes returned by a checked-read session. Exceeding
any limit refuses the complete
inventory or checked read; callers never receive a partial snapshot or partial
member bytes.

## Exhaustive kind vs three coarse projections

These mappings are documentation only. There is no conversion helper, no fifth
enum, and no migration.

| `JournalEntryKind` | `DirEntryKind` | `ConflictKind` | `NoFollowEntryKind` |
|--------------------|----------------|----------------|---------------------|
| `RegularFile` | `File` | `RegularFile` | `RegularFile` |
| `Directory` | `Directory` | `Directory` | *(no variant)* |
| `Symlink` | `Other` | `Symlink` | `Symlink` |
| `Fifo` | `Other` | `Other` | `Other` |
| `Socket` | `Other` | `Other` | `Other` |
| `CharacterDevice` | `Other` | `Other` | `Other` |
| `BlockDevice` | `Other` | `Other` | `Other` |
| `Other` | `Other` | `Other` | `Other` |

`DirEntryKind` collapses a symlink into `Other` because `list_dir_entries` uses
`std::fs` `is_file` / `is_dir` on `DirEntry::file_type()` (lstat).
`NoFollowEntryKind` has no directory arm by design;
`ConflictKind::as_wrong_kind` is `None` for `Directory`.

`JournalEntryKind::from_mode` is the single `SFlag` match. Archive
`classify` / `classify_mode` call it.

## Canonical path is metadata

The path stored on `JournalRoot` is the verified spelling at admit time. It is
not source authority. After an ancestor rename the retained descriptor still
binds the original object; the stored spelling does not change.

Uses: archive manifest `source_journal`, export default path, and
`reject_export_tree_output`. Access via `ArchiveSource::canonical_source`.

## Archive reuse

`ArchiveSource` holds exactly one `JournalRoot`. On Unix, inventory, `open_file`,
and proof revalidation walk descendants through `AsFd`. On Windows, it freezes
the witnessed journal-io inventory after applying the same portable deny policy;
exact member reads delegate to journal-io's witnessed, retained-relative checked
read and return complete verified bytes rather than a raw handle. In both cases,
`ArchiveSource::open` maps `JournalRootError` exhaustively onto `ArchiveError`
(`InvalidJournal`, `UnsupportedJournal`, `SourceIo` with `member: None`,
`SourceChanged` with `member: None`).

## Unsupported

`JournalRootError::Unsupported` is the explicit refusal for an unsupported
backend policy; it is never a silent path-only mode. The Unix backend never
emits it. Windows gate 1 uses it for reparse, filesystem, drive-type, and
Cloud Files policy refusals; each carries a `WindowsRefusalCategory`.
`CloudSyncRootStatusUnverifiable` carries the returned raw HRESULT when one
exists, while `CloudSyncRootRegistered` denotes S_OK; `ReFS` roots do not
issue a Cloud Files query. Ordinary permission and I/O failures remain
`JournalRootError::Io`.

## Future-backend obligations

Windows covers root admission, complete witnessed source enumeration, route
revalidation, checked archive-source reads, portable name admission, and
durable `append_jsonl`.
Atomic publication, retention, packaging, archive encoding and publication,
`flat_directory`, `snapshot`, `staged`, `health_marker`,
`append_text`, and `claim_remove` remain explicitly Unix-only and unsupported
on Windows in this slice.

A later backend must: admit once; retain an opaque identity; revalidate that
object rather than reopen by path; surface the same four refusals; forbid
`Clone` and serialization; treat any stored path as metadata. No public
filesystem trait, no `Box<dyn>`, no path-only fake backend.

## Bound publication

`atomic_replace_bound`, `write_bytes_exclusive_bound`, `acquire_existing_parent_lock_bound`,
`create_directory_bound`, `read_bytes_bound`, and `sync_dir_bound` operate on a caller-supplied
directory descriptor and a single normal name. They never open a parent via `AT_FDCWD`
and never treat a stored pathname as source authority. `BoundAtomicOutcome` is `Published` or
`PublishedDurabilityUncertain` only; pathname-identity outcomes stay on `atomic_replace_detailed`.
`acquire_existing_parent_lock_bound` uses `AsFd` on Unix and has a real `AsHandle` implementation
on Windows; the other bound APIs remain Unix-only and `AsFd`-only.
`acquire_existing_parent_lock_bound` returns `BoundParentLock`, which has no `path()`.
Descendant walks remain the caller's (archive inventory).
`is_day_key` is the single 8-digit day-key predicate.

Bound publication stages use names that cannot be confused with their
destination's leading visibility convention: an ordinary destination stages as
`.tmp_<pid>_<sequence>.tmp`, while a dot-prefixed destination stages as
`_tmp_<pid>_<sequence>.tmp`.

## Flat-directory capability

`FlatDirectory` is a retained descriptor for one nonempty portable descendant
directory below `JournalRoot`. Each component is admitted and opened with
`O_NOFOLLOW` relative to the previous descriptor. Its diagnostic path is
metadata, not authority. It operates on direct children only: no recursive
walk, path reacquisition, or `AT_FDCWD` fallback.

`list_flat_directory` returns sorted no-follow entry observations or `None`
when the entry count exceeds its supplied bound; it never returns a partial
list. `FlatDirectoryEntry` carries name, kind, device/inode, size, and native
mtime precision. `read_observed_file` returns exact bytes only after regular
kind, device/inode, size, and mtime agree before and after the read. Atime and
ctime are deliberately not comparison fields.

## Claimed removal

`ClaimName` is caller-owned and exactly
`!solstone-claim-<8 lowercase hex>-<16 lowercase hex>`. Leading `!` is
outside product stream grammar even under case- or normalization-insensitive
comparison. Callers own uniqueness, scanning, and retained-claim handling;
claim names are visible.

`claim_and_remove_observed` accepts a caller-supplied original name, prior
observation, and claim name. It first rejects an aliasing claim, then uses a
required no-replace rename into the claim slot. It unlinks only a matching
claimed entry through the retained directory descriptor. It never removes the
original name after a pathname stat.

Outcomes are `Removed`, `RemovedDurabilityUncertain`, `Unchanged` (occupied
claim slot, unsupported primitive, or reconciled no-op), and `IdentityChanged`
(`Restored`, `RetainedClaim`, or `UnknownLocation`) with durability evidence.
Original disappearance after a valid observation is `UnknownLocation`, never
benign absence. Ambiguous rename errors are reconciled by no-follow observation
of both direct names before any deletion decision.

An already-open descriptor can still mutate the claimed object. This API does
not provide hard-link ownership proof, advisory locking, persistent claim
registry or cleanup, claim-name hiding, recursive removal, Windows support,
heartbeat behavior, or archive refactoring.

Windows `claim_and_remove_observed` is explicitly unsupported. The published
Win32 and NTFS documentation establishes collision behavior, POSIX-style
deletion, and metadata caching, but not the one property the Unix state
machine depends on: an atomic transfer of the observed original into an
absent claim name that holds under concurrent namespace creators, with
directory-level durability.

[`FILE_RENAME_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info)
documents a handle-relative rename destination via `RootDirectory` and that a
false `ReplaceIfExists` errors when the target exists, collision behavior,
not a documented atomic transfer into an absent claim name under a concurrent
creator.
[`SetFileInformationByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle)
documents the handle-information operation itself and warns that behavior can
differ by information class across OS releases; it supplies no missing
concurrency guarantee for the claim step, and it is also the source of the
`ReFS` support statement addressed below. Once a claim exists,
[`FILE_DISPOSITION_INFORMATION_EX`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information_ex)
documents POSIX deletion semantics, closing the delete handle removes the
visible link while existing handles remain usable, but this is a post-claim
deletion mechanism that cannot repair the undocumented atomic claim step that
has to happen first.
[`File caching`](https://learn.microsoft.com/en-us/windows/win32/fileio/file-caching)
states that filesystem metadata is cached, which is exactly why an
undocumented durability barrier is disqualifying rather than incidental.

[`CreateFile`'s write-through documentation](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea)
names rename metadata among the `NTFS` metadata changes
`FILE_FLAG_WRITE_THROUGH` flushes. That is an `NTFS`-specific metadata-flush
fact, not a documented atomic no-replace transfer under concurrent namespace
creation, and it has no `ReFS` counterpart: `SetFileInformationByHandle`'s
statement that the operation is supported on `ReFS` is support, not a
durability proof, and none of these five documents supplies an equivalent
`ReFS` durability guarantee. The missing atomic claim transfer is
independently sufficient to keep this fail-closed on both `NTFS` and `ReFS`;
the `NTFS` metadata-flush fact neither supplies that missing concurrency
property nor extends to an equivalent `ReFS` durability guarantee. Windows
therefore does not substitute a path-based claim, overwrite-capable rename,
or uncertain delete for the Unix state machine.

## Append JSONL

`append_jsonl` is available on Unix and Windows. It serializes one record,
adds one newline, performs one append write, and requires `File::sync_all` to
succeed before it returns success. A write error can still leave a partial
record if the operating system reports a short write; callers must treat every
error as indeterminate on-disk state.

On Unix, a newly created record file also receives the existing best-effort
parent-directory sync. Windows has no equivalent directory-handle sync in this
surface, so a Windows success means the record file flush completed; it does
not claim durable parent-directory entry creation. `append_text` remains
Unix-only because this lane exposes only the JSONL primitive used by
Callosum's Windows-compilable default surface.

## No-replace platform support

Linux uses `renameat2` with `RENAME_NOREPLACE` through the Linux syscall ABI;
this covers GNU and musl targets. macOS uses descriptor-relative
`renameatx_np` with `RENAME_EXCL`. Unsupported no-replace primitives or volumes
return an explicit unchanged outcome; they never fall back to overwrite-capable
rename.

## Lock sidecar naming

`derive_sidecar_path` in `solstone-core-journal-io` appends `.lock` to
`file_name()` as native `OsString` bytes. Ordinary UTF-8 spelling is unchanged
(`health.sqlite` → `health.sqlite.lock`). Distinct native basenames are distinct
lock authorities.

The carrying release must quiesce the old service and process set before
starting the new binary. Old inert lossy-named `*.lock` sidecars are left in
place and are harmless once no old binary holds them. Code alone does not make
mixed-version rollout safe.

## Collision-scan non-atomicity

Name-admission collision scan is not atomic with a later create: there is no
lock file, and a concurrent raw-filesystem writer (including `segment_path` /
talent-runtime `DEFAULT_STREAM`) can still plant a colliding name between scan
and mutation.

## Strict segment admission

`preflight_segment_admission`, `create_segment_strict`, `resolve_stream_exact`,
`resolve_segment_exact`, and `resolve_segment_locator_exact` are a
strict-admission / exact-read preparatory API with stable pre-existing checks.
They are not authoritative across namespace races until the following
root-bound caller cutover. They are not Windows support.

`RecordIdentity` / `record_identity()` is the legacy sentinel: Direct spells as
`_default`, and a literal Named `_default` directory is unrepresentable and
refused. `SegmentLocatorIdentity` / `locator_identity()` is lossless: it always
carries an explicit `SegmentLayout` alongside the stream spelling, so Direct
and Named-`_default` are never conflated. Disk occupancy alone cannot recover
which layout produced an already-written `_default`-stream record. That
record has no retained layout tag, so callers of
`resolve_segment_locator_exact` must supply `SegmentLayout` rather than have
it inferred.

## Convey Shell segment identity

Convey Shell builds one private, fallible catalog from journal I/O
`day_dirs`, `iter_segments`, `Segment::locator_identity`, and
`resolve_segment_locator_exact`. A catalog row carries the day, explicit
layout, exact UTF-8 stream, exact segment-directory basename, parsed time key,
and the validated discovered path. A traversal, identity, or exact-resolution
failure fails the catalog instead of producing a partial inventory.

At historical Shell request and durable-record boundaries, a missing
`stream_layout` means Named. New Shell responses and records emit either
`direct` or `named`; an unknown or malformed tag is never treated as Named.
`segment_key` remains the exact directory basename. Parsed time metadata is a
separate value, so a basename such as `093000_300_summary` is never rebuilt as
`093000_300` for lookup.

Read surfaces admit both layouts through the explicit resolver. Shell-owned
speaker mutations whose downstream implementation is still layout-blind
refuse Direct before any mutation. The bundled speaker workspace keys a row by
the JSON-encoded tuple `(day, stream_layout, stream, exact basename)`, and only
accepts a legacy basename-only deep link when that basename is unique among
the loaded rows.

This does not complete the speaker-resolve migration. That crate still has
layout-blind bootstrap, backfill, identify, resolve, owner, and voiceprint
paths built on the legacy `segment_path` contract or records without an
explicit layout. Direct segments remain unsupported whenever a Shell command
hands control to one of those Named-only mutation paths.
