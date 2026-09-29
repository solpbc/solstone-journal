# Journal Format Contract Maintenance

## Adding a client with a new format

Define a new schema whose floor, the `$defs.header` and `$defs.record`
`required` arrays, captures only what every producer of that format can meet.
Put producer-specific requirements such as `raw` at the producer: the writer
code should emit the field, and a producer test should pin that invariant.
Never put producer-specific requirements in the shared floor.

Browser admission validates the strict snapshot/delta record union. Stored
browser JSONL and the historical reader are not re-gated by those new required
fields. Audio and screen remain headered JSONL. This schema is the exception to
the `$defs.header` / `$defs.record` floor rule.

Register the schema, then run:

```bash
make contract
make check-contract
```

## Forward-compatibility governing principle

The ingest contract is a published interface consumed by native clients.
Adding a `required` field is an intentional, deliberately-made, documented
breaking change because it rejects existing producers. It requires a forward
maintenance migration or a coordinated producer upgrade. There is no
version-negotiation layer.

Relaxing the floor by removing a `required` field is forward-compatible and
safe. `raw` was relaxed from the floor for exactly this reason: producers
with no source media can legitimately omit it while producers that own `raw`
continue to pin it locally.

## Crate-owned schemas

Schemas with dedicated admission validation (such as `browser.schema.json` owned
by `solstone-core-ingest-contract`) embed the authoritative schema document
directly in the owning crate, expose compiled validator routines, and report
a schema digest embedded into the contract bundle.

