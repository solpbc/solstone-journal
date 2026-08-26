# Journal Format Contract Maintenance

## Adding a client with a new format

Define a new schema whose floor, the `$defs.header` and `$defs.record`
`required` arrays, captures only what every producer of that format can meet.
Put producer-specific requirements such as `raw` at the producer: the writer
code should emit the field, and a producer test should pin that invariant.
Never put producer-specific requirements in the shared floor.

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
