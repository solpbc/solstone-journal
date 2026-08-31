# MCP bridge PoP v1 raw corpus provenance

- Bridge source commit: `ea5de0136e4fa24f4ad092a2756178b197ed50e7`
- Initial exchange source: `crates/spl-bridge/src/pop_auth.rs`
- Renewal exchange source: `crates/spl-bridge/tests/journal_lease_renewal.rs`
- Regeneration control: `git -C /var/tmp/mcp-bridge-source.KjLyJj show ea5de0136e4fa24f4ad092a2756178b197ed50e7:crates/spl-bridge/src/pop_auth.rs`

The deterministic raw fixtures below deliberately use an artificial bearer
token and an Ed25519 fixture key. They establish byte framing and the exact
proof input only; no account token, hostname, bridge address, or key material
from a journal is present here.

```text
initial-frame-hex:
0000003b7b22746f6b656e223a22666978747572652d746f6b656e222c22686f73746e616d65223a2261616171656179652e736f6c73746f6e652e6d65227d

challenge-frame-utf8:
\x00\x00\x00\x56{"nonce":"AAECAwQFBgcICQoLDA0ODw","bridge_id":"bridge-fixture","timestamp":1700000000}

proof-input-hex:
000102030405060708090a0b0c0d0e0f6272696467652d66697874757265000000006553f100
```
