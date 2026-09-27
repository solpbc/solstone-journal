# Release Evidence

Scope: what this repository uses to publish a journal release and to record
the evidence for it. Read this before changing `solstone-distribution publish`,
the transparency tool pin, the v2 origin release registry, or the per-release
origin pins.

## Publishing a release set

`solstone-distribution publish`
(`core/crates/solstone-core-distribution/src/publish.rs`) copies a built release
set into one lane: `release`, `staging` or `dev`. Before it writes anything it
verifies the manifest signature and validates every member of the release set
against the manifest. It refuses a set whose product is not `solstone-journal`
or that has no `solstone-journal-<version>-<target>.release` member.

In `release` and `staging`, an object that is already published is left as it
is; only `dev` overwrites. The lane's `latest` file names a version and never
moves to an older one.

## Transparency

Release records live on the shared transparency rail,
[solpbc/solstone-transparency](https://github.com/solpbc/solstone-transparency),
at `software/journal/<version>/release-record.json`. This repository runs that
rail's tools only at a pinned revision.
`core/distribution/solstone-transparency-pin.json` names the repository commit
and the SHA-256 of each executable, and
`core/crates/solstone-core-distribution/src/transparency.rs` checks both before
it runs anything.

- `solstone-distribution journal-artifacts --transparency-repo DIR -- ARGS...`
  runs the pinned `journal-artifacts` adapter with `ARGS`.
- `solstone-distribution register-v2-origin --transparency-repo DIR --root FILE
  --version VERSION --store FILE --apply` runs the pinned `verify-release`
  against the published record. Only when it verifies does the command record
  the version, the record's path and its SHA-256 in
  `core/crates/solstone-core-origin/v2-release-registry.json`.

`core/crates/solstone-core-origin/src/v2_registry.rs` reads that registry and
refuses an unknown schema, an unsafe version, a malformed digest or a repeated
version.

## Origin pins

`core/crates/solstone-core-origin/pins/v<version>.json` lists, for one release,
each origin artifact it depends on: the origin key, its SHA-256, and the unit it
belongs to. `pins.rs` compiles every listed release's pins into the crate. The
retention guard (`guard.rs`) refuses by default to prune an origin artifact
that a supported release still pins.

A test (`snapshot_files_and_transparency_log_are_bijective` in
`core/crates/solstone-core-origin/src/tests.rs`) requires the pinned versions to
equal the supported releases: the versions in `transparency-head-log.jsonl` plus
the versions in the v2 origin release registry. A new release therefore adds its
pin file, its `pins.rs` entry and its registry entry together.

## Related

- `AGENTS.md` § Release and transparency
- `docs/journal-format-contract-maintenance.md`: the sibling discipline for
  journal at-rest formats
