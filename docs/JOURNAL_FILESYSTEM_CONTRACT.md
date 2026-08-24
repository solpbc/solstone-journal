# Journal filesystem contract

This is the shared vocabulary for a journal root, its identity, entry kinds, and
refusals. It is not a generic VFS. It is not Windows support. The only backend
today is Unix: both crates target-gate `nix` (archive also target-gates
`solstone-core-journal-io`) and each refuses non-Unix with its own `compile_error!`.
`core/ci/windows-crosscheck.toml` excludes both as sibling-owned platform backends.

## Root, identity, kind, refusal

`solstone-core-journal-io` owns:

- **`JournalRoot`:** one admitted journal directory, retained by descriptor.
- **`ObjectIdentity`:** opaque `(device, inode)` pair. No public constructor,
  no accessors for the raw pair, no serde.
- **`JournalEntryKind`:** exhaustive no-follow kind: `RegularFile`,
  `Directory`, `Symlink`, `Fifo`, `Socket`, `CharacterDevice`, `BlockDevice`,
  `Other`.
- **`JournalRootError`:** `Invalid`, `Unsupported`, `Io`, `Changed`.

`JournalRoot` is not `Clone`: a cloned descriptor would be a second capability,
and a cloned path would be reacquisition. It is not serializable.

## Unix retained handle and no-follow

Admit once. The retained directory descriptor is source authority. Do not
reopen the root by path.

The canonical walk uses `O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW`. The
requested-root open omits `O_NOFOLLOW` so a symlink root is allowed. Descendant
opens in archive keep `O_NOFOLLOW` (regular files also `O_NONBLOCK`).

Revalidate the admitted object (`fstat` of the retained descriptor against the
frozen identity, and confirm it is still a directory). Do not walk the stored
canonical path to reacquire.

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

`JournalRootError::Unsupported` is the explicit refusal for a backend that
cannot retain a handle. The Unix backend never emits it. It is not a silent
path-only mode.

## Future-backend obligations

A later backend must: admit once; retain an opaque identity; revalidate that
object rather than reopen by path; surface the same four refusals; forbid
`Clone` and serialization; treat any stored path as metadata. No public
filesystem trait, no `Box<dyn>`, no path-only fake backend. Windows policy,
Win32, and `windows-sys` are out of scope. The current build continues to
exclude these Unix-only crates from Windows cross-checks.

## Bound publication

`atomic_replace_bound`, `write_bytes_exclusive_bound`, `acquire_existing_parent_lock_bound`,
`create_directory_bound`, `read_bytes_bound`, and `sync_dir_bound` operate on a caller-supplied
directory descriptor (`AsFd`) and a single normal name. They never open a parent via `AT_FDCWD`
and never treat a stored pathname as source authority. `BoundAtomicOutcome` is `Published` or
`PublishedDurabilityUncertain` only; pathname-identity outcomes stay on `atomic_replace_detailed`.
`acquire_existing_parent_lock_bound` returns `BoundParentLock`, which has no `path()`.
Descendant walks remain the caller's (archive inventory, convergence store).
`is_day_key` is the single 8-digit day-key predicate.

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
