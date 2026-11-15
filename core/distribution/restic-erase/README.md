# restic-erase

The journal's restic helper for removing an erased thing from every backup.
It is restic 0.19.0 (upstream commit `12875cc48ed34111f71fe94a24a47ad11660f3a4`,
the same source the iOS bridge and the Windows ARM64 build pin) with one patch,
[`erase-paths.patch`](erase-paths.patch), which adds two hidden commands and
changes nothing else:

- `erase-paths --request R --state S`, run under one exclusive repository lock:
  1. **find** every snapshot that holds the named paths;
  2. **save** the ids of the pieces they use to `S`, durably, before anything
     changes, merged with any list a stopped run saved;
  3. **rewrite** each of those snapshots without the paths, keeping time, host,
     tags and paths, then forget the old one;
  4. **remove** the listed pieces no snapshot still uses, rewriting only the
     pack files that hold them; a piece still in use stays and is reported;
  5. **scan** every pack file the backend lists, reading each header, and fail
     while any copy of a removed piece remains. A failed delete is an error.
- `scan-blobs --state S`: the same physical scan, read-only.

Stock restic remains the program for backup, check and restore.

## Qualifying it

For every restic upgrade, rebuild the helper on the new source and run:

```sh
python3 -I qualify.py --stock /path/to/restic --helper /path/to/restic-erase
```

Every case uses synthetic data in a fresh repository under `/var/tmp`, and
stock restic is the outside check: `check --read-data` passes, every
remaining snapshot restores byte for byte except what was erased, and the
erased pieces, listed independently with stock `restic cat tree`, are found
before and absent after. The cases cover both paths an erased thing sits at,
a straddling backup and the outputs beside a tombstone, an earlier trash copy,
a shared piece, a stop after each phase (with the same and a larger retry),
two runners at once, a racing backup in both orders, a refused and a falsely
acknowledged delete, media archives by exact path, and an entry no backup held.
