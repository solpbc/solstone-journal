# Journal filesystem contract

This is the shared vocabulary for a journal root, its identity, entry kinds, and
refusals. It is not a generic VFS. `solstone-core-journal-io` supports Unix and
Windows journal-root admission; `solstone-core-journal-archive` remains Unix-only.

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

Windows gate 1 admits only a journal root and its portable final name. It opens
the exact requested path with `CreateFileW` using `FILE_FLAG_BACKUP_SEMANTICS |
FILE_FLAG_OPEN_REPARSE_POINT`. A reparse point at the root or any inspected
ancestor is refused; unlike Unix, this gate does not admit a leaf symlink or
junction.

Admission has two passes: an authoritative open captures attributes and
`FileIdInfo`, then an independent absolute-path walk opens every ancestor and
the target again, rejecting reparses and comparing the final target identity.
The retained handle is the only identity authority. `revalidate()` reads that
handle only and never reopens a path.

Filesystem admission is a strict `NTFS`/`ReFS` allow-list, and volume admission
is a strict fixed-drive allow-list; each wildcard refuses rather than admits.
`NTFS` roots undergo Cloud Files classification; `ReFS` roots are admitted
without a `CfGetSyncRootInfoByPath` call. For `NTFS`, a registered sync root
refuses; HRESULTs `ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` (390) and
`ERROR_NOT_A_CLOUD_FILE` (376) admit; every other HRESULT refuses as
unverifiable. `GetDriveTypeW` and `CfGetSyncRootInfoByPath` are
classification-only queries against the already-validated path spelling, never
identity reacquisition; identity comes exclusively from `FileIdInfo` on the
retained handle.

> **Known Windows gate-1 limitation:** the Win32 surface used here has no
> descriptor-relative open equivalent to Unix's `openat` + `O_NOFOLLOW` chain,
> so the ancestor verification uses separate absolute-path opens. The final
> target identity recheck catches final-target replacement, but cannot prove an
> intermediate ancestor was not transiently swapped and restored between those
> independent opens. This is materially weaker than Unix's descriptor-relative
> walk and is a known gate-1 limitation, not equivalent-strength authority.

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

`ArchiveSource` holds exactly one `JournalRoot`. Inventory, `open_file`, and
proof revalidation walk descendants through `AsFd`. `ArchiveSource::open` maps
`JournalRootError` exhaustively onto `ArchiveError` (`InvalidJournal`,
`UnsupportedJournal`, `SourceIo` with `member: None`, `SourceChanged` with
`member: None`).

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

Windows gate 1 covers root admission and portable name admission only.
Locking, leases, atomic publication, retention, packaging, archive,
`flat_directory`, `snapshot`, `staged`, `health_marker`, `append`, and
`claim_remove` remain explicitly Unix-only and unsupported on Windows in this
slice.

A later backend must: admit once; retain an opaque identity; revalidate that
object rather than reopen by path; surface the same four refusals; forbid
`Clone` and serialization; treat any stored path as metadata. No public
filesystem trait, no `Box<dyn>`, no path-only fake backend.

## Bound publication

`atomic_replace_bound`, `write_bytes_exclusive_bound`, `acquire_existing_parent_lock_bound`,
`create_directory_bound`, `read_bytes_bound`, and `sync_dir_bound` operate on a caller-supplied
directory descriptor (`AsFd`) and a single normal name. They never open a parent via `AT_FDCWD`
and never treat a stored pathname as source authority. `BoundAtomicOutcome` is `Published` or
`PublishedDurabilityUncertain` only; pathname-identity outcomes stay on `atomic_replace_detailed`.
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
