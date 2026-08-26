# Convey Client Ingest OpenAPI Contract

`docs/openapi/convey-clients.json` is the hand-maintained authority for native
client routes. The linked-device ingest bundle is generated from it by
`solstone-core-repository-contracts`.

## Scope

The bundle covers exactly the four Rust-served linked-device ingest operations:

| Method | Path | operationId |
|---|---|---|
| POST | `/app/devices/ingest` | `client.ingestUpload` |
| GET | `/app/devices/ingest/manifest` | `client.ingestManifest` |
| GET | `/app/devices/ingest/manifest/{day}` | `client.ingestManifestDay` |
| GET | `/app/devices/ingest/segments/{day}` | `client.ingestSegments` |

Pairing and root SSE routes remain live but are intentionally outside this
bundle. Settings capture configuration remains `settings.observe.*`; it is a
separate capture-settings surface and is not part of client identity.

## Generated bundle

The committed artifacts live under `docs/openapi/client-ingest-contract/`:

- `manifest.json`
- `projection.openapi.json`
- `vectors.json`
- `fixtures/wire-behavior.json`
- `consumer-audit.json`

`core/crates/solstone-core-repository-contracts/src/contracts/client_ingest_contract_bundle.rs`
owns the projection and artifact generation. The manifest identifies this as
`solstone.client-ingest-contract-bundle.schema.v1`, uses the
`solstone.repository_contracts.client_ingest_contract_bundle.v1` generator
identity, and records the `client_protocol_version` plus artifact digests.

The consumer audit can retain literal legacy v2 external surface names such as
`observer_v2_register`: those are historical evidence, not current operation
identifiers.

## Updating the bundle

1. Edit `docs/openapi/convey-clients.json` directly.
2. Regenerate the committed artifacts with:

   ```text
   cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts client_ingest_contract_bundle::regenerate_client_ingest_contract_bundle -- --ignored
   ```

3. Run the normal repository-contract library tests. The
   `generated_bundle_matches_committed_files` test verifies the authority and
   committed artifacts agree.

Bundle SemVer is separate from the authority document version. Bump it for
breaking bundle changes, including operation-id or generated-identity changes.
