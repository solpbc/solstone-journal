# W11 Importer Reachability Design

## Purpose and boundary

W11 makes the already-landed native importer grammar reachable as `journal
importer` through `solstone-core`. It adds only the parsed-command forwarding
seam, the thin binary handler, argv dispatch, pure CLI rendering, and
spawned-binary reachability tests. It does not cut over the retained Python
process row; that is W12.

The scope's description of three unlanded bodies is stale. Generic audio,
archive merge, and all four chat/reading export parsers have landed. The W7
source-crate stub table now contains only `registry`, `apple_health`, and
`oura`; `chatgpt`, `claude`, `gemini`, and `kindle` are landed bodies. The
source crate still depends on `solstone-core-import`, however, so making
`cli_argv` call it directly would introduce a Cargo dependency cycle. Until a
dedicated adapter owns that direction, every explicit registry-source route
validates through the resolver and then refuses by source name; it is a uniform
crate-boundary refusal, not an unimplemented-body claim or fabricated preview.

The boundary is explicit:

- `cli_argv` owns argv parsing, the supervisor-gate seams, and dispatch to
  already-landed import bodies.
- `cli_render` owns pure data-to-string rendering: full help, importer-table
  JSON and human forms, backends, and owner-facing sync/connect/refusal
  shaping. It performs no I/O and accepts no argv.
- The `main.rs` handler only validates UTF-8, skips journal resolution for
  help, resolves the journal otherwise, calls `cli_argv::run_cli`, writes the
  returned stdout/stderr, and returns the returned code; it contains no
  orchestration, detection, staging, or mode logic.

The supervisor gate belongs inside `cli_argv`, not the thin handler. The
required order is help short-circuit, parse argv, supervisor gate, then mode
dispatch (including the missing-media error). `run_cli_with(args, journal,
lookup_env, connectivity)` already exposes exactly the two seams needed to
preserve it, and `require_solstone_with` supplies the live contract: success
when skip is set or connectivity succeeds, silent spawned-unavailable refusal,
or interactive unavailable refusal. A handler-level gate would incorrectly
turn a down-supervisor `--nonsense` invocation into exit 1 before parsing
instead of the gate-passed parser exit 2.

## One roster for reachability assertions

`importer_reachability.rs` will declare one named `MODE_CASES` roster. The
criterion-1 fail-first pass, criterion-1a observable pass, and criterion-1c
negative-twin pass all iterate that same roster; mode-specific subcases remain
members of their parent mode. No pass receives its own hand-maintained subset.
All argv below are passed to a freshly spawned
`env!("CARGO_BIN_EXE_solstone-core")`; source paths are test-created inputs.

| Mode | Gate-passed argv / subcases | Observable assertion |
|---|---|---|
| 1. Generic media path | `TEXT YYYYMMDD_HHMMSS`; `AUDIO YYYYMMDD_HHMMSS`; `AUDIO --dry-run` | Text calls the native transcript processor and audio calls the native audio importer, both using test-created media and keeping stderr empty. Audio dry-run returns a named preview-path refusal because the landed audio body has no preview surface; it never claims a preview occurred. Generic media with no timestamp returns a named timestamp-detection-adapter refusal rather than pretending auto-detection ran. |
| 2. Structured sources | `--source ics|obsidian|document|image|journal_archive|chatgpt|claude|gemini|kindle SOURCE` | Each route first resolves the real input, then exits 1 with empty stdout and a named `native importer cannot invoke the <Source> source body` refusal. This is a dependency-boundary refusal, not an unimplemented-body claim or synthetic success. |
| 3. Body/Apple native return | `--source apple_health APPLE_EXPORT --dry-run --json` | Exit 0 on stdout; parsed JSON identifies `schema: solstone.body.ingest.result.v1`, `source: apple_health`, and preview mode. Stderr is empty. |
| 4. Body/Oura file-route refusal | `--source oura OURA_FILE` | Exit 1, empty stdout, and exactly `Oura body data imports through sync; use journal importer --sync oura` on stderr. |
| 5. Importer listing | `--list-importers`; `--list-importers --json` | Both exit 0 with empty stderr. Human stdout equals the fixture block; JSON stdout parses and deep-equals the grammar fixture's `importers` array in its eleven-row order. |
| 6. Backends | `--backends` | Exit 0, empty stderr, and stdout equals the fixture block containing Plaud, Obsidian, Audio, then Oura. |
| 7. Sync | `--sync audio --path AUDIO_DIR`; `--sync obsidian --path VAULT`; `--sync plaud`; `--sync oura`, with `--save`, `--window-days`, `--scheduled`, and consent controls only where applicable | Audio and Obsidian previews call their filesystem-backed sync bodies and report the scanned source path and catalogue count. Plaud calls its body and reports the real missing-credential refusal when no credential adapter is configured. Oura uses its native body. Save requests that lack a landed pipeline or note-import adapter refuse by name rather than changing a rendered mode word. |
| 8. Connect | `--connect oura` and `--connect unknown` | The controlled Oura route has the native owner-present outcome on its documented stream and never exit 64; the unknown-backend control exits 1 on stderr with `Unknown connect backend: unknown` and the connectable-backends line. |
| 9. Positional journal-source sub-CLI | `journal-source create NAME`, `journal-source list`, `journal-source status NAME`, `journal-source revoke NAME` | Each command exits with its `cli_journal_source::run_cli` result on its documented stream; the create/list/status/revoke fixture sequence uses a test journal and is identifiable by the journal-source record/name. `journal-source --help` is separately an outer-help fixture case because help short-circuits before positional dispatch. |

The fail-first run uses this same roster against a binary without the importer
dispatch arm: every listed case must produce exit 64 with the top-level usage
on stderr. The live reachability pass then asserts the table's observables and
the negative twin for every roster member: no exit 64 and no top-level usage
banner on stderr. Listing, backend, sync, connect, and journal-source cases do
not borrow resolver-corpus assertions; that corpus covers only resolution.

For native argv resolution, filesystem manifest lookup is wired through
`find_manifest_by_hash` for known generic media. A matching manifest produces a
skip unless `--force` is present, so `--force` has a real deduplication effect.
Registry claims cannot cross the source-crate dependency boundary, and native
deterministic/model timestamp detection has no landed adapter. An unclassified
media path therefore refuses by name, and generic media without an explicit
timestamp refuses by name instead of silently treating `--auto` or detection
controls as no-ops.

## Grammar and the 6a decision

`media` is an optional positional at parse time, followed by an optional
timestamp positional. The parser accepts this full HEAD grammar:

| Input | Arity / status |
|---|---|
| `-h`, `--help` | flag; short-circuits before journal resolution and supervisor checking |
| `media` | optional positional |
| `timestamp` | optional second positional; historical shorthand for `--timestamp` |
| `--timestamp`, `--facet`, `--setting`, `--source`, `--sync`, `--path`, `--window-days`, `--connect`, `--date-from`, `--date-to` | one value; `--window-days` parses an integer |
| `--auto` | optional value; bare form is enabled, value form supplies guidance |
| `--force`, `--dry-run`, `--confirm-body-save`, `--with-day-summaries`, `--deterministic-only`, `--backends`, `--save`, `--scheduled`, `--list-importers`, `--json`, `-v`/`--verbose`, `-d`/`--debug` | flags |
| `--confirm-health-save` | accepted alias of `--confirm-body-save`, sharing one destination |

`--with-day-summaries` and `--confirm-health-save` are accepted but
SUPPRESS'd from HEAD help. `--confirm-body-save` is the visible spelling. The
HEAD parser is authoritative for accepted grammar and intended visibility; the
grammar fixture is authoritative for the pinned registry order, presentation
data, alias assertion, suppression list, and terminal output data. Where the
committed help oracle contradicts HEAD, it remains the byte oracle for the
blocked 1b assertion rather than becoming a new grammar authority.

**Decision: reject surplus positionals.** `MEDIA TIMESTAMP EXTRA...` exits 2
with the importer usage block followed by `journal importer: error:
unrecognized arguments: EXTRA...`. No committed fixture pins this case: the
help oracle's `unknown_option` and the grammar fixture's
`unknown_option_exit_codes` concern an option, not surplus positionals. The
reference silently absorbs extras only as a side effect of `parse_known_args`,
not as an intentional affordance. Retaining that behaviour could quietly drop a
second media path while importing only the first. `CLAUDE.md` §8's project rule
is to fail loudly rather than silently, while preserving the explicitly required
two-positional timestamp shorthand and rejecting unknown options.

Unknown options are also rejected at parse time, before the supervisor gate,
and therefore exit 2 in both supervisor columns with importer usage plus an
`unrecognized arguments:` error. This deliberately diverges from the retained
reference's down-supervisor row, which gates first and exits 1. The native
interpreter-poison probe runs in that down column and requires exit 2; its
environment does not set the skip flag. The grammar fixture's summary note
describes only its gate-passed row, while its measured down row says exit 1, so
the rows rather than that summary establish the reference behaviour.

## Supervisor columns and fixture strategy

The test helper owns environment selection through a `SupervisorColumn` enum;
individual tests supply a column and argv, never raw environment mutations.
That helper derives both the fixture key (`<column>/<case>`) and process
environment, making a fixture-column/environment mismatch unrepresentable:

- `gate_passed` sets `SOL_SKIP_SUPERVISOR_CHECK=1` and clears the spawned flag.
- `solstone_down` clears both flags and uses a pinned unreachable journal.
- `supervisor_spawned` clears skip, sets `SOL_SUPERVISOR_SPAWNED=1`, and uses
  the same unreachable journal.

Thus an unknown option is a deliberate pre-gate exception: both down-supervisor
and gate-passed invocations exit 2 with importer usage and an
`unrecognized arguments:` error. A valid parsed invocation still reaches the
gate, and spawned-and-unavailable returns exit 75 with both stdout and stderr
genuinely empty. `check_no_python_spawn.rs` is only a spawning/poisoning style
reference: it runs `check`, not `importer`, and supplies no importer
supervisor-column precedent.

The importer table is authored in `cli_render` from the eleven fixture-defined
display names, patterns, and descriptions. Tests load
`core/fixtures/import_reference_grammar.json` and deep-equal its `importers`
array; they must not restate it in a Rust constant. The human rendering is also
fixture-exact. In particular, `journal_archive` exceeds the twelve-character
field width, so its row has one separating space rather than aligned padding.

## Blocked help fidelity

Criterion 1b is blocked on a corrected fixture capture. The affected cases are
`help_long`, `help_short`, and `journal_source_help`, in both supervisor
columns. `unknown_option` is a deliberate divergence and is asserted against
the native decision above, never against the fixture. The fixture provenance identifies a reference revision
whose `solstone/think/importers/cli.py` suppresses `--with-day-summaries` and
`--confirm-health-save` while exposing `--confirm-body-save`; its rendered help
does the reverse for all three. Argparse cannot render a SUPPRESS'd option, so
the capture did not execute the source file whose digest it records.

The suite will nevertheless key byte-for-byte assertions by fixture case name:
the binary's stdout/stderr and exit must equal the named record, with no
substring relaxation. Those assertions remain expected to fail until a
corrected capture lands, then become green without changing test expectations.
This wave neither regenerates, reconstructs, patches, nor locally renders the
fixture.

## File sequence and checks

1. Add `Importer(Vec<OsString>)` parsing/dispatch in
   `core/crates/solstone-core-cli/src/lib.rs`, add the importer usage line, and
   add `"importer"` to `usage_lists_supported_commands`. The latter is a
   hand-written coverage list: adding the token is correct; restoring a frozen
   prose snapshot of `USAGE` is expressly not.
2. Add the direct `solstone-core-import` dependency and the thin `run_importer`
   adapter in `core/crates/solstone-core/src/main.rs`, matching
   `run_storage_ops_verb`.
3. Replace `cli_argv` and `cli_render` reserved seams with the boundary above;
   route each parsed mode to an already-landed body where reachable and preserve
   named crate-boundary refusals for source bodies that cannot be reached.
4. Add `core/crates/solstone-core/tests/importer_reachability.rs`. It uses the
   spawned freshly built binary for every criterion, has no
   `required-features = ["differential"]`, and therefore runs on a bare
   checkout through `make ci`.
5. Keep all W12 cutover work separate: no process-table/probe mutation, no
   Python change, no source-crate/audio change, no native `sol import` change,
   and no generated-inventory change.

## Risks and handoff

The largest current risk is the unusable help fixture; 1b cannot honestly be
reported green until its capture is corrected. Separately, the direct-binary
suite proves only binary reachability. It does not prove the W12 dispatcher
argv composition, sibling-binary resolution, or sibling-interpreter poisoning;
those remain explicit W12 handoff tests.
