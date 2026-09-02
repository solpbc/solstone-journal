# MCP bridge PoP v1 raw corpus provenance

- Bridge source commit: `ea5de0136e4fa24f4ad092a2756178b197ed50e7`
- Initial exchange source: `crates/spl-bridge/src/pop_auth.rs`
- Renewal exchange source: `crates/spl-bridge/tests/journal_lease_renewal.rs`
- Regeneration control (from an `spl-rust` checkout): `git show ea5de0136e4fa24f4ad092a2756178b197ed50e7:crates/spl-bridge/src/pop_auth.rs && git show ea5de0136e4fa24f4ad092a2756178b197ed50e7:crates/spl-bridge/tests/journal_lease_renewal.rs`

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

renewal-frame-hex (fixture successor token, fixture Ed25519 seed byte 19):
000000a47b22746f6b656e223a22666978747572652d737563636573736f72222c22686f73746e616d65223a2261616171656179652e736f6c73746f6e652e6d65222c227369676e6174757265223a22376c5236716d38307769716b5a767465434639354672497734416b5a315950387a5f424379553836597330416f41466b6365304a6a77714d776d74586855646d7576367473744b5073446b2d69724c6d337a41704177227d
```
