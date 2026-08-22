# Journal device simulator

This dependency-free Python utility behaves like a linked capture device. It
sends digest-pinned fixture bytes through the maintained native `solstone link`
transport, reconciles each submission through journal ingest v3, and writes
machine-readable evidence.

The simulator does not implement PL, SPL, TLS, pairing, or journal storage.
Those remain native product boundaries. It also does not replace each platform
client's own integration tests.

## Quick start

Validate the bundled text-only smoke corpus without starting a journal:

```bash
python3 -m tools.journal_device_sim validate \
  --manifest tools/journal_device_sim/fixtures/smoke/manifest.json
```

Run against a disposable journal pairing window over PL-direct:

```bash
python3 -m tools.journal_device_sim run \
  --manifest tools/journal_device_sim/fixtures/smoke/manifest.json \
  --profile smoke \
  --carrier direct \
  --pair-code 'PAIR-LINK-URL' \
  --convey-port JOURNAL_CONVEY_PORT \
  --journal-root /path/to/disposable/journal
```

The simulator stores native credentials under the run's private state
directory, starts `solstone link serve --port 0 --direct`, reads the assigned
loopback port from the native startup line, and owns only that child process.

Unless `--bridge-url` is supplied, the native launcher defaults to the
source-built `core/target/debug/solstone` in this checkout. Build it first with
`make build`. The simulator never falls back to `PATH`: `--solstone-bin` is
only accepted as an absolute override path, and bare or relative values are
rejected. Before pairing, validating a pre-paired bundle, or starting a native
bridge, it runs the selected launcher with `--help` and requires stdout byte 0
to start with `solstone - journal access CLI`. This preflight is bounded and
fail-closed; if it fails, recover by running `make build` or passing
`--solstone-bin /abs/path/to/solstone`.

For relay testing, `--carrier relay` invokes `solstone link serve --port 0
--relay-only`. This policy excludes LAN endpoints even when the credential
bundle contains them. The simulator records owned bridges as
`native-direct-only` or `native-relay-only`. A caller-supplied `--bridge-url`
must be a literal loopback origin and is recorded as `caller-asserted`, because
the simulator does not own or inspect that bridge's process arguments.

`--convey-port` selects the local journal served by the native child. When it is
omitted, the child uses `SOLSTONE_CONVEY_PORT` if present, then the native
default. The effective port and its source are included in evidence.

If pairing must happen before the journal switches to SPL, use one private
state directory for both phases:

```bash
python3 -m tools.journal_device_sim pair \
  --pair-code 'PAIR-LINK-URL' \
  --state-dir scratch/journal-device-sim/relay-proof \
  --convey-port JOURNAL_CONVEY_PORT

# Switch the disposable journal to SPL through its supported management surface.

python3 -m tools.journal_device_sim run \
  --manifest tools/journal_device_sim/fixtures/smoke/manifest.json \
  --profile smoke \
  --carrier relay \
  --paired \
  --state-dir scratch/journal-device-sim/relay-proof \
  --convey-port JOURNAL_CONVEY_PORT \
  --journal-root /path/to/disposable/journal
```

The pair link appears in the simulator and native child's command-line
arguments, so treat shell history and local process listings as sensitive. The
simulator does not echo the link or write it to state or evidence; the
[`test_native_child_owns_pairing_ephemeral_port_and_cleanup`](tests/test_process.py)
and
[`test_owned_carrier_assurance_waits_for_native_startup`](tests/test_runner.py)
regressions pin those output boundaries. `--paired` requires an explicit state
directory and refuses absent, incomplete, or linked credential state. A
passing run removes credentials unless `--keep-credentials` is set. Other
outcomes retain them for controlled retry or diagnosis.

## Verification levels

Each profile declares one `verification` level:

- `contract` exercises the public v3 POST, listing, day-manifest, root-manifest,
  receiver identity, and carrier-posture contracts. It does not require local
  journal filesystem access. If `--journal-root` is supplied, custody checks run
  too.
- `custody` requires `--journal-root`. It binds the public listing to the exact
  authenticated device event and selected segment directory, then either
  streams and hashes the retained raw file or checks an exact terminal
  processing record.
- `processing` requires `--journal-root` and one closed processing expectation
  per submitted file. It checks the successful processing record, exact input
  size, derived sidecar, and every semantic output row. Empty terminal output
  can satisfy custody but cannot satisfy processing.

External bridges need `--expected-cid sha256:...` whenever `--journal-root` is
used. Simulator-owned bridges derive that CID from the exact client certificate
used by native `link serve`.

## Fixture manifest

The manifest schema is `solstone.journal-device-sim.fixtures.v1`. Every file
names its relative fixture path, submitted filename, byte size, and SHA-256.
Loading fails before transport if bytes drift, a path escapes the fixture root,
or a submitted name belongs to the journal itself.

```json
{
  "schema": "solstone.journal-device-sim.fixtures.v1",
  "profiles": {
    "smoke": {
      "segments": ["tmux-alpha"],
      "verify_duplicate": true,
      "verification": "contract"
    }
  },
  "segments": [
    {
      "id": "tmux-alpha",
      "day": "20260201",
      "segment": "080000_30",
      "source": "tmux",
      "files": [
        {
          "path": "alpha.jsonl",
          "submitted": "tmux.jsonl",
          "size": 42,
          "sha256": "lowercase sha256"
        }
      ],
      "meta": {"fixture": "synthetic-benign"},
      "expect": {
        "upload_statuses": ["ok"],
        "file_statuses": ["present"]
      }
    }
  ]
}
```

Processing expectations are closed over the native media contract. Audio
inputs use `transcribe`; screen-video inputs use `describe`; and the output is
the submitted filename with its extension replaced by `.jsonl`. For example,
`sample.wav` maps to `sample.jsonl`.

`--date-mode shift` maps the fixture's final day to today while preserving
relative offsets. `--anchor-day YYYYMMDD` makes that mapping deterministic.
`--date-mode preserve` sends the authored dates.

## Recovery and evidence

Before each send, the simulator persists `phase: sending`. A transport error or
5xx response is ambiguous, so the simulator reconciles the exact submitted
name, size, digest, requested key, and landed key before deciding whether to
retry. A received response that violates the v3 contract is a terminal failure,
not an ambiguous transport result.

Outcomes are `PASS`, `FAIL`, `BLOCKED`, or `INCONCLUSIVE`. Evidence includes:

- fixture and simulator source provenance;
- the native launcher selection mode, exact executable path, digest, and
  available version string;
- effective Convey targeting and carrier assurance;
- the client certificate digest, derived device CID, and non-secret peer
  identity;
- bounded response receipts, accepted-response metadata, reconciliation reads,
  request counts, and journal oracles.

Evidence excludes raw non-JSON or oversized response bodies, simulator-held
private keys, attestation tokens, relay tokens, pair links, and receiver-status
endpoint fields. The
[`test_received_malformed_responses_are_failures_with_safe_receipts`](tests/test_runner.py),
[`test_prepaired_provenance_exposes_only_non_secret_credential_fields`](tests/test_process.py),
and
[`test_receiver_status_evidence_projects_private_endpoints`](tests/test_runner.py)
regressions pin those projections.

## Field journal

Clone the public
[`field_journal`](https://github.com/solpbc/field_journal) beside this repository.
Use it only as a fixture source, and upload into a different disposable journal.

```bash
python3 -m tools.journal_device_sim field-manifest \
  --field-root ../field_journal \
  --output scratch/journal-device-sim/field-manifest.json
```

The generator accepts only Git-tracked raw audio and screen-video files named
by the field journal manifest. Every raw format becomes its own simulated
segment so derived sidecars cannot alias one another. It emits:

- `field-smoke-custody` and `field-smoke-processing` for representative format
  coverage;
- `field-large` for the exact 19,200,078-byte WAV custody and duplicate canary;
- `field-full-custody` and `field-full-processing` for the complete corpus.

Run the generated manifest with `--fixture-root ../field_journal`. Custody and
processing profiles also require the receiving `--journal-root`; processing
runs normally need a suitable `--processing-timeout` and installed media
providers.

## Tests

```bash
make check-journal-device-sim
```

The standard-library test suite covers fail-closed manifest handling, streaming
multipart framing, typed HTTP response failures, ambiguous-response
reconciliation, collision and duplicate behavior, carrier invocation,
filesystem confinement, semantic processing checks, cleanup, and resumable
state. Live PL and SPL checks run separately against disposable journals.

Malformed protocol-version and multipart cases remain in the Rust ingest
handler's boundary tests:
[`legacy_fields_and_protocol_versions_are_refused`](../../core/crates/solstone-core-ingest/src/router.rs)
and
[`protocol_validation_distinguishes_every_version_refusal`](../../core/crates/solstone-core-ingest/src/validation.rs).
The native bridge's v3 forwarding and relay-only policy are covered in
[`sol_link_serving.rs`](../../core/crates/solstone-core-sol-link/tests/sol_link_serving.rs)
and
[`command.rs`](../../core/crates/solstone-core-sol-client/native/think/link/command.rs).
