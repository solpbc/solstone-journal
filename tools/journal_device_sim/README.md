# Journal device simulator

This repository-local, dependency-free Python utility behaves like a linked
capture device. It sends explicit fixture bytes through the maintained native
`solstone link` transport, reconciles each submission through journal ingest v3,
and writes machine-readable evidence.

It does not implement PL, SPL, TLS, pairing, or journal storage. Those remain
native product boundaries. It also does not replace the independent platform
integration gates: a simulator exercises the journal's composed ingest path, while
Linux, macOS, iOS/watchOS, Android, Windows, and tmux retain their own integration
gates.

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
  --journal-root /path/to/disposable/journal \
  --processing-timeout 30
```

Use `--carrier relay` for SPL. For a bridge already owned by another harness,
replace `--pair-code` with `--bridge-url http://127.0.0.1:PORT`. Bridge URLs are
restricted to literal loopback origins. The simulator does not query carrier
identity from an external bridge, so its evidence marks carrier assurance as
`caller-asserted`; simulator-owned native children are marked `native-child`.

The paired mode stores credentials under the run's private state directory,
starts `solstone link serve --port 0`, reads the bound port from its startup
line, and terminates only that child. Credentials are removed after a passing
run unless `--keep-credentials` is set. Failed or inconclusive runs retain state
so the same command and `--state-dir` can reconcile before retrying.

## Fixture manifest

The manifest schema is `solstone.journal-device-sim.fixtures.v1`. Every submitted
file names its exact relative path, submitted filename, byte size, and SHA-256.
Loading fails before transport if any byte has drifted, a path escapes the
fixture root, or a client attempts to submit a journal-authored sidecar such as
`stream.json`, `ingest.json`, or `events.jsonl`.

```json
{
  "schema": "solstone.journal-device-sim.fixtures.v1",
  "profiles": {
    "smoke": {
      "segments": ["tmux-alpha"],
      "verify_duplicate": true
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

`--date-mode shift` maps the fixture's final day to today while preserving
relative offsets, so multi-day processing fixtures do not land in the future.
`--anchor-day YYYYMMDD` makes that final-day mapping deterministic.
`--date-mode preserve` sends the authored dates.

The default accepted custody states are `present` and `processed`. A fixture may
require named derived outputs with `expect.required_outputs`; that requires the
read-only `--journal-root` check. Profiles verify those outputs by default and
may set `verify_processing: false` when the intended proof is transport custody
only. When a journal root is supplied, the simulator always requires the
server-authored `stream.json`, `ingest.json`, and `events.jsonl` for every
accepted segment.

## Recovery and evidence

Before every send, the simulator writes `phase: sending` to `state.json`. If the
connection closes or the server returns 5xx after bytes land, it queries
`/app/devices/ingest/segments/{day}` and matches the exact submitted name, size,
and SHA-256 before retrying. The runner opens fixture payloads read-only; its
manifest and field-manifest tests cover path confinement and input provenance.

Evidence outcomes are `PASS`, `FAIL`, `BLOCKED`, or `INCONCLUSIVE`. The evidence
records the manifest digest, fixture repository revision when available, date
mapping, carrier, request count, response, and reconciled listing. `state.json`
and `evidence.json` omit pair links and credential material. In paired mode,
native credentials live only under `<state-dir>/credentials` and are removed
after a passing run unless `--keep-credentials` is set.

## Field journal

Clone the public
[`field_journal`](https://github.com/solpbc/field_journal) beside this repository,
then use it as a read-only fixture root and upload into a different disposable
receiving journal. A field manifest must enumerate exact raw input paths and
digests; do not directory-sweep a field segment because the fixture tree may
also contain server-authored or derived files. The real 19,200,078-byte WAV at
`journal/20260201/field.audio/080000_600/audio.wav` is the routine bridge-limit
canary.

Generate the explicit adapter manifest into simulator state without modifying
the field journal:

```bash
python3 -m tools.journal_device_sim field-manifest \
  --field-root ../field_journal \
  --output scratch/journal-device-sim/field-manifest.json
```

The generator accepts only Git-tracked `audio.{flac,m4a,ogg,opus,wav}` and
`screen.{mp4,mov,webm}` files at paths named by the field journal's own 90-entry
manifest. It creates `field-smoke` (source/format/modality coverage),
`field-large` (the 19.2 MB custody canary with duplicate proof), and `field-full`
profiles. `field-smoke` and `field-full` additionally require their named
derived outputs; `field-large` deliberately remains usable when a disposable
sandbox has no media model. Run them with the generated manifest plus
`--fixture-root ../field_journal`; add `--journal-root` and a suitable
`--processing-timeout` for processing profiles.

## Tests

```bash
make check-journal-device-sim
```

The tests are standard-library-only and exercise manifest fail-closed behavior,
streaming multipart framing, ambiguous-response reconciliation, collision and
duplicate behavior, and resumable state. Live PL/SPL validation remains an
operator lane because it requires a real pairing window and carrier.

Malformed protocol-version and multipart-contract cases stay in the Rust ingest
handler's boundary tests: see
[`legacy_fields_and_protocol_versions_are_refused`](../../core/crates/solstone-core-ingest/src/router.rs)
and
[`protocol_validation_distinguishes_every_version_refusal`](../../core/crates/solstone-core-ingest/src/validation.rs).
The native bridge supplies the protocol header named by
[`bridge_names`](../../core/crates/solstone-core-sol-link/src/serve.rs), so varying
that header in the simulator would bypass the maintained transport boundary this
tool exercises.
