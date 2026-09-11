# Talent fault scenarios

This finite scenario runner exercises the native talent worker with scripted
Generate and Cogitate responses. It checks preparation, subprocess requests,
bounded retries, output disposition, and terminal events together.

Run it from the repository root with an explicitly built native binary and its
matching payload:

```bash
python3 -m tools.talent_fault_sim \
  --binary /absolute/build/solstone-core \
  --payload /absolute/checkout/core/payload \
  --field-root /absolute/field_journal \
  --field-ref COMMIT \
  --output /absolute/new-run-directory
```

The output directory must not exist. The runner creates private journals and
a copied payload there. A test-only sibling executable answers model calls;
other commands are refused. No production fault switches, credentials, or
network model calls are used. Inputs remain unchanged.

The fixture comes from the pinned Git object for
`reference/ami/ES2002a/transcript.txt` in
[field_journal](https://github.com/solpbc/field_journal). It uses the first 1,800
characters, normalizes whitespace, and writes a transcript row into each test
journal. This bypasses media transcription. The AMI transcript is attributed
through the source repository's `ATTRIBUTION.md`, copied into the evidence
directory. Synthetic probe talents exercise runtime mechanics; this is not a
content-quality evaluation of shipped talents.

Scenarios cover clean Generate and Cogitate results, schema and JSON-length
recovery, exhausted schema retry, Cogitate refusal, and publication failure
after a valid result from either engine. The checks require exact call counts,
fixture content in assembled Generate input, Cogitate journal identity,
intermediate retry evidence, one final talent outcome, and expected artifact
bytes. Publication failures cover an unreadable existing artifact and an invalid
destination. The runner verifies that its current account cannot read the
permission fixture and refuses the run if that precondition does not hold.

Every run keeps raw events, child requests, scripted responses, stderr,
artifact digests, and per-scenario verdicts. Provenance records fixture revision
and digest, binary digest, payload hashes, and the probe overlay. Hashes identify
the supplied inputs; the caller must independently establish that the binary
was built from the intended source revision.

The runner exits nonzero on any failed scenario or setup error. Each worker has
a deadline and independent stdout/stderr limits. Timeout, output overflow, and
interruption terminate the owned process group. This initial harness requires
Unix. Keep runs outside routine unit CI; its focused oracle and process-bound
checks are available with:

```bash
make check-talent-fault-sim
```

These tests deliberately corrupt call counts, inputs, terminal events, causes,
artifact bytes, and retry evidence to confirm that the checks fail. They also
exercise output overflow and a timed-out descendant holding a pipe.

The runner does not start Cortex or the scheduler, exercise real tool side
effects, establish source freshness, or score model quality. Those require
separate scenarios. Its saved fault schedules provide a starting point for
restart tests and seeded random faults as those boundaries gain coverage.
