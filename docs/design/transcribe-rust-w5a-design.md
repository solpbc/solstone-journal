# W5a: native `journal transcribe` design

## Decision record

W5a builds the Python transcribe driver's standalone native replacement. The
resulting process boundary is intentional: `solstone-transcribe` will become
the production seam called by `journal transcribe` in a later, macOS-VAD-
packaging-gated dispatch wave. It is therefore not an inspection/debug tool.
W5a does not change the live Python route.

The Python source remains the differential oracle during the conversion freeze.
The native implementation owns transcription orchestration and the two
owner-media deletions currently performed in Python. It does not change
`solstone-core-system`; that crate has no public client API for the
parakeet-cpp port or placement records.

## 1. Crates, binary, and workspace

Add these two crates:

- `core/crates/solstone-core-transcribe`: library package
  `solstone-core-transcribe`. It has `publish = false`, workspace lints, and
  **no** `[[bin]]` target.
- `core/crates/solstone-core-transcribe-cli`: binary package
  `solstone-core-transcribe-cli`. It has `publish = false` and one `[[bin]]`
  named `solstone-transcribe`, at `src/main.rs`.

The CLI does **not** receive `package.metadata.solstone-release.skip`. Unlike
the brain CLI's standalone inspection bed, this binary is the production
transcription seam and must ship with the journal host, matching retention's
seam-oriented shape.

`solstone-core-transcribe` has this normal, unconditional direct dependency
list and no other helper-crate dependency:

- `solstone-core-observe-audio`
- `solstone-core-system`
- `solstone-core-speaker-id`
- `solstone-core-processing-record`
- `solstone-core-callosum`
- `solstone-core-journal-config`
- `solstone-core-journal-io`

It must never list `solstone-core-vad-analyze`,
`solstone-core-speakers-analyze`, or `solstone-core-speakers-onnx` in a
dependency table, including as optional or feature-gated dependencies. The
crate may retain a dependency-free `differential` feature solely to gate its
Python-oracle test targets; it has no `real-vad` or `real-speakers-analyze`
feature.

Register the pair using the retention four-line form exactly: consecutive
workspace members for `crates/solstone-core-transcribe` and
`crates/solstone-core-transcribe-cli`, and consecutive
`[workspace.dependencies]` entries for `solstone-core-transcribe` and
`solstone-core-transcribe-cli`. The CLI depends on the library through the
workspace dependency.

W5a does **not** touch `PROCESS_SPECS`, the journal process table, Python
dispatch code, or journal-distribution packaging. The standalone binary is
reachable only through its crate tests, the AC16 end-to-end reachability test
that spawns its built path directly, and direct manual/CI invocation. It is
never reached through live `journal transcribe` routing in this wave.

## 2. Library module boundary

`solstone-core-transcribe/src/lib.rs` exposes only the CLI-facing request/run
contract, typed `TranscribeError`, exit-code conversion, and model-resolution
error. All stage implementation is private.

Private modules:

- `args.rs`: grammar-neutral parsed CLI input (`<audio-path>`, `--all`,
  `--redo`, `--backend`) and journal-relative path handling.
- `config.rs`: strict journal config extraction, including the
  `confidential_audio` default/invalid-value rule.
- `stage.rs`: single-file stage machine, batch accounting, input metadata
  capture, and outcome ownership.
- `audio.rs`: decode/VAD/reduction composition using
  `solstone-core-observe-audio` and a local VAD-helper subprocess adapter; it
  does not move `speech_ratio` into either dependency.
- `backend.rs`: backend selection and dispatch gate.
- `backend/parakeet_cpp.rs`: Linux HTTP client, transport classification,
  server health/port/placement reads, and Parakeet response parsing.
- `backend/confidential.rs`: confidential transport and its reason vocabulary.
- `model_assets.rs`: the new source/installed/override model resolver.
- `speakers.rs`: locates and spawns the sibling
  `solstone-core-speakers-analyze` binary, writes one JSON request to stdin,
  reads one JSON response line from stdout, maps its 64/69/75 exits and stderr
  JSON error lines into the driver's error taxonomy, then validates response
  payloads and performs cleanup. It retains the current bounded helper
  invocation semantics.
- `transcript.rs`: header construction, in-process
  `solstone_core_speaker_id::writer::write_request` request adaptation, and
  orphan-sidecar handling.
- `terminal.rs`: direct header-only terminal JSONL publication and terminal
  processing records.
- `processing.rs`: construction of `_solstone_processing` values from
  `solstone-core-processing-record` vocabulary.
- `event.rs`: content-free `observe.transcribed` envelope and Callosum emit.

`core/crates/solstone-core-transcribe-cli/src/main.rs` handles stdin-free CLI
argument parsing, resolves the journal in the same host-authority manner as
other native journal tools, invokes the library, prints the existing human
batch summary, and returns the library's deliberate exit code. It contains no
pipeline behavior.

### ORT isolation and helper-process contracts

Both ONNX-backed helpers are sibling processes, not Cargo library
dependencies. `audio.rs` locally reimplements the small
`resolve_vad_binary` shape: `SOLSTONE_VAD_BINARY`, when nonempty, is the
explicit path; otherwise it uses the directory containing
`current_exe()` and its `solstone-core-vad-analyze` sibling. It sends the VAD
JSON request on stdin, accepts one JSON response line on stdout, and maps the
helper's 64/69/75 exits and stderr JSON error line into the driver's typed
taxonomy.

`speakers.rs` uses the same deterministic sibling rule with
`SOLSTONE_SPEAKERS_ANALYZE_BINARY` and the sibling name
`solstone-core-speakers-analyze`. For transcription it invokes that binary
with **no arguments**, selecting its `Command::Run` path; it does not pass
`discovery-cluster`, which is the sole argv spelling that selects
`Command::DiscoveryCluster`. Its JSON stdin/stdout/stderr and 64/69/75 exit
contract is handled as described above.

This deliberately corrects the original scope's in-process
`speakers-analyze` wording. `solstone-core-speakers-analyze` already provides
the equivalent subprocess binary, so its public contract can be used without
reshaping or reimplementing its internals. More importantly,
`ci_gate_purity::rust_host_excludes_match_the_workspace_onnx_closure` computes
the ONNX closure from syntactic dependency keys and does not distinguish
optional dependencies or feature selection. A dependency edge to either helper
would mechanically require transcribe in `RUST_HOST_EXCLUDES` and the iOS
exclude chain, contrary to W5a. The standalone helpers remain isolated by the
existing exclusions; transcribe has no ONNX dependency in any feature
configuration.

## 3. Model assets

`model_assets.rs` is new production code. It resolves an **asset directory**,
then verifies the requested named regular file is nonempty. It never asks
Python or executes a subprocess.

Resolution order is fixed:

1. If `SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR` is set, use that directory only.
   It must contain the requested asset; an invalid override is an immediate
   typed error and does not fall through to a different location.
2. Source checkout: for every ancestor of compile-time
   `CARGO_MANIFEST_DIR`, test
   `packages/solstone-journal-models/solstone_journal_models/assets`. This is
   the cargo-test/source-build route only.
3. Installed journal environment: obtain `current_exe()`, iterate it and its
   ancestors, and for each candidate environment root test every child matching
   `lib/python3.*/site-packages/solstone_journal_models/assets`. The expected
   normal venv layout is
   `<venv>/bin/solstone-transcribe` and
   `<venv>/lib/python3.<minor>/site-packages/solstone_journal_models/assets`;
   scanning the `python3.*` child derives the minor version rather than
   hard-coding one. The package manifest explicitly installs
   `assets/*.onnx` as `solstone_journal_models` package data.

The error type records `OverrideInvalid`, `CurrentExecutable`, and
`AssetNotFound { asset, searched }`. It is a hard configuration failure
(exit 78) because selecting an STT/VAD model cannot honestly defer work. The
existing `CARGO_MANIFEST_DIR` examples are test-only references, not a
production precedent.

## 4. Stage machine and terminal records

`stage.rs` preserves the four logical stages and the current two terminal
empty-output branches:

1. Resolve/validate input and capture `InputFacts { path, input_size }` using
   `metadata().len()` **before VAD and before any possible unlink**.
2. Decode; on decode failure write a terminal failed header-only JSONL.
3. Run VAD, compute `speech_ratio` locally as
   `speech_duration / duration` (zero duration produces `0.0`), and run
   best-effort sound tagging.
4. On VAD no-speech, publish terminal empty JSONL then conditionally delete
   raw media. Otherwise reduce audio when applicable, select/gate/dispatch the
   backend, then either publish terminal empty JSONL for zero STT statements or
   analyze speakers and publish the full transcript/NPZ result.

`processing.rs` builds the terminal record at the terminal decision point,
using `solstone_core_processing_record::vocab` constants for schema, states,
reasons, and handler. It includes UTC `attempted_at`, the pre-captured
`input_size`, and failed-attempt accounting where applicable. The record is
inserted into the JSONL header as `_solstone_processing`.

`terminal.rs` and the later full-transcript path in `transcript.rs` both build
the same `solstone-speaker-transcript-write-request-v1` JSON request and call
`solstone_core_speaker_id::writer::write_request(bytes)`. The request carries
`schema`, `output { jsonl_path, npz_path, redo }`, `base_time_us_of_day`,
`source`, `statements`, `header`, and `embeddings`. The real writer owns
preflight, redo/`DestinationExists`, staging, and atomic publication; terminal
publication must not reimplement describe's `promote()` shape.

For either terminal write, `statements` is `[]`, yet `embeddings` remains a
complete writer payload: a real temporary zero-byte f32le file at
`payload_path`, `payload_format: "raw-f32le-row-major-v1"`,
`dtype: "float32-le"`, `shape: [0, 256]`, `byte_count: 0`, empty
`statement_ids` and `durations_s`, and
`encoder: "wespeaker-resnet34-256"`. This exactly follows Python's shared
writer wrapper, which defaults an empty embedding payload to that `ENCODER_ID`.
The request must still supply `npz_path`, although the writer's zero-row mode
does not publish an NPZ. Create the empty payload immediately before the call
and remove it regardless of the result.

`processing.rs` still relies on `solstone-core-processing-record` only for
vocabulary and predicates; that crate has no writer API. Its completed value
is placed directly in the shared writer request's `header` as
`_solstone_processing`, which `build_header` preserves verbatim in the JSONL
header. The writer call succeeds completely before either owner-media unlink.
The exact captured `input_size` therefore binds the subsequent
`evaluate_terminal_proof` check to the raw bytes that were removed.

## 5. Output sidecars and writer errors

Reproduce `_remove_orphan_npz`. `transcript.rs` provides one helper that, when
the sibling NPZ exists but the JSONL does not, removes that NPZ before **every**
transcript publication attempt: both terminal paths and the analyzed path. An
unlink failure is `orphan-npz-remove-failed` and exit 75. This preserves retry
behavior and prevents an abandoned NPZ from blocking either terminal or full
publication.

The six subprocess-bound reasons are retired by linkage and must not be
manufactured: `unsupported-host`, `handshake-skip`, `handshake-fail`,
`launch-failed`, `invalid-response`, and `payload-tempfile-failed`. There is no
handshake, helper launch, or response parsing when calling `write_request` in
process. The terminal request's local empty payload file is nevertheless
required by the writer's input validation; its creation or cleanup failure is
a typed internal failure, not the retired `payload-tempfile-failed` reason.

Map the actual `SpeakerTranscriptWriteError` variants as follows:

- Exit 69: `PayloadUnreadable`, `PayloadInvalid`, `PayloadNonFinite`.
- Exit 75: `OutputUnwritable`, `NpzVerificationFailed`, `Internal`, and the
  driver's own orphan-NPZ-removal error.
- Exit 1: `MalformedRequest`, `UnknownSchema`, `MissingStatementId`,
  `InvalidStatementId`, `DuplicateStatementId`, `InvalidStatement`,
  `InvalidHeader`, `InvalidOutputPath`, and `DestinationExists`.

The mapping is matched on the writer enum variants, not on free-form strings;
only the corresponding stable `reason()` value is placed in telemetry. This
includes every present writer variant and deliberately has no compatibility arm
for retired Python-bound reasons. It applies identically to both terminal empty
writes and the full analyzed write because all three call the same writer
function. AC24 therefore injects typed and untyped writer failures at Site A
and Site B against one writer contract, rather than two publication mechanisms.

The temporary zero-byte terminal embedding payload is not owner media and is
not part of either terminal-media removal site's accounting. It is distinct
from `_remove_orphan_npz`, which handles a real persisted NPZ sibling left by a
prior writer attempt when its JSONL is absent.

## 6. Parakeet C++ and backend selection

`backend/parakeet_cpp.rs` owns two small first-party journal readers:

- `health/parakeet-cpp.port`: read text, trim, parse `u16`; absent, unreadable,
  empty, or invalid means no port.
- `health/parakeet-cpp.placement`: read text and trim; accept only `cpu` or
  `gpu`, otherwise no placement.

It then performs the same loopback health probe and classifies any exception or
non-200 as `server_not_ready`; no `solstone-core-system` API is added or used.
Placement overrides configured device in model/event metadata, as it does in
Python.

Backend selection has two distinct predicates that must remain separate:

- **Routing predicate** in `resolve_default_backend`: confidential routing is
  allowed only when the confidential channel is usable *and*
  `confidential_audio_enabled` is true. This selects confidential versus local
  placement/surface before a file is dispatched.
- **Dispatch refusal predicate** immediately before **every** backend dispatch:
  `confidential_provenance` is present. Under that predicate, permit registered
  local backends; permit `confidential` only when confidential audio remains
  enabled; refuse any remote/unregistered backend. If confidential was selected
  while provenance is now absent, defer as `confidential_lane_inactive`.

This second gate is placed after reduction and before the backend call, not
merged into routing, so a configuration/provenance change between selection and
dispatch cannot cause audio egress.

## 7. Events and exit behavior

`event.rs` retains the documented five outcomes and all 20 event-schema fields.
It constructs content-free fields only; `error` is an exception/error class,
never a diagnostic message. The current 11-row defer/fail policy and 28-row
reason table are the source contract.

All terminal writes return typed results. A failed terminal write never reaches
the subsequent unlink. The stage runner maps typed provider deferrals to 69,
typed transient/native failures to 75, installation/model configuration faults
to 78, and hard failures to 1. Batch mode absorbs only per-file 69, increments
the deferred count, and rethrows all other exits, matching the current Python
batch behavior.

## 8. Dependency-policy amendment

Add `solstone-core-transcribe` to the `solstone-core-journal-io` wrapper list
in `core/deny.toml`. Replace the current final clause of its `reason` string:

> and the retention executor -- the sole remover of owner media -- in this wave

with this literal text:

> and the retention executor -- the sole remover of owner media except for the transcribe executor's two terminal-media removal sites, which are guarded by terminal processing proof -- in this wave

This is an amendment to the ownership statement, rather than a misleading
append that still says retention is sole remover.

## 9. Differential and gate plan

The native crate declares a `differential` feature. Add only its Python-oracle
integration targets with `required-features = ["differential"]`; each is owned
and invoked as `-p solstone-core-transcribe --test <target>`.

- `transcribe_differential`: no real ONNX VAD requirement; compares fixture
  stage/output/event/exit behavior under stub VAD/STT/speaker seams.
- `transcribe_vad_differential`: drives the real VAD helper/model against the
  Python VAD/reference path.

Add `transcribe_differential` as its own ordinary loop leg in
`check-differentials`, with the package name above. Put
`transcribe_vad_differential` after that loop as its own ORT-env-prefixed,
status-accumulating leg, alongside (not merged into) the existing
`solstone-core-vad-analyze` VAD leg. It uses the same
`VAD_ANALYZE_HOST_ORT_ENV` staging prerequisite and remains isolated so ORT
environment variables do not leak to other differential legs.

Add a separate non-differential `transcribe_stub_vad_reachability` test to the
crate's ordinary test set. It injects a stub VAD seam and spawns/exercises the
native stage reachability path without linking or provisioning ONNX Runtime;
therefore it runs in `make ci`. This is the AC38 split: CI proves the native
driver is reachable, while the differential rail proves real VAD parity.

The transcribe manifest has no ONNX-helper dependency in any configuration.
The standalone VAD and speakers-analyze binaries are the only ORT-linked
components used by this design, and their existing exclusions remain unchanged.
Do not add transcribe to `RUST_HOST_EXCLUDES` or the 22-crate iOS exclusion
list. The AC38 stub-VAD reachability test supplies a test-injected fake helper
binary; the real-VAD differential resolves the real VAD helper at runtime.
The same pattern may supply a stub speakers-analyze binary to AC16.

`ci_gate_purity` will mechanically require each `required-features =
["differential"]` target to appear in `check-differentials`, and each quoted
leg must name this owning package.

## 10. Ordered implementation sequence

1. Add crate manifests, workspace registration, standalone binary build
   entries, and the `deny.toml` wrapper amendment.
2. Establish the library's typed errors, CLI request/run contract, asset
   resolver, config parsing, and non-ORT stub seams.
3. Implement input facts, terminal record/header publication, orphan cleanup,
   and proof-before-unlink behavior before any backend work.
4. Port VAD/reduction/speech-ratio and Parakeet health/transport/placement
   behavior; add confidential routing and the separate dispatch refusal gate.
5. Port speakers-analyze adaptation, full transcript writer adaptation, writer
   enum-to-exit mapping, and event emission.
6. Add the non-ORT reachability test, differential manifests/tests, Makefile
   package-owned legs, and the isolated ORT VAD differential leg.
7. Make no Python dispatch, `PROCESS_SPECS`, or journal-distribution packaging
   changes in this wave. The later macOS-VAD-packaging-gated dispatch wave owns
   that cutover; it must not retain a fallback shim when it lands.

## Risks requiring preservation during implementation

- Terminal write and raw unlink must remain one-directional: no unlink after a
  failed or unproven output write.
- The current driver distinguishes Site A's formerly untyped escape from Site
  B's generic conversion. The native implementation must make both writes
  typed, preserving their externally documented exit behavior rather than
  preserving accidental Python exception mechanics.
- The installed resolver depends on distribution layout. It must test actual
  regular files and report every searched candidate, not infer success solely
  from an executable name or a Python minor version.
- The speakers helper's 2400-second process budget, stream limits, response
  validation, and temp cleanup are part of failure classification; simplifying
  them would change telemetry and retry semantics.
- `solstone-core-speaker-id` currently publishes NPZ before JSONL, so an NPZ
  can remain after a later JSONL failure. The driver-level orphan cleanup is
  therefore required on every retry path.
- The Rust-conversion freeze permits temporary old/new routing only with a
  deletion schedule. W5a intentionally creates no old/new live routing; the
  later native journal-dispatch wave owns deleting the retained Python route,
  without a compatibility alias or fallback subprocess.
