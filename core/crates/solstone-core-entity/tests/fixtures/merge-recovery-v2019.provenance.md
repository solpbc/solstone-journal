# merge-recovery-v2019 fixtures

Journal trees written by solstone-journal v2.0.19 (tag `v2.0.19`, commit
`9e48db83970ca659e56062f535b4b757fcd9fc4e`), so recovery of records written
by that build is tested against that build's own bytes and fingerprints.

- `merge-before`, `merge-interrupted`: a merge of `source` into `target` that
  ran every phase and then failed to commit, on an injected sync failure of
  the journal root. `merge-interrupted` holds the uncommitted recovery record.
- `undo-before`, `undo-committed`: that merge's undo, failed by the `edges`
  injector after its source commit. `undo-committed` holds the committed
  `"operation": "undo"` record.

Generated on Linux with umask 0022 by a one-off test added to v2.0.19's
`core/crates/solstone-core-entity/src/merge_tests.rs`; its source is in
`merge-recovery-v2019.generator.rs.txt`.

Git keeps neither file modes nor empty directories, and recovery fingerprints
both. `merge-recovery-v2019.modes` lists every path with its mode, and tests
recreate the directories and apply the modes before use. Never regenerate
`expected.json`.
