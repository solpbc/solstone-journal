# Synthetic LAN qualification client

This development driver uses SPL v0.11.1 pairing, TLS pinning, request framing and response assembly. Platform harnesses send JSON commands on stdin and receive one JSON reply on stdout. Each operation has a 30-second deadline. Run it only against a synthetic journal on the intended local test network.

```sh
cargo build --locked --manifest-path tools/mobile-serving/client/Cargo.toml
tools/mobile-serving/client/target/debug/ios-serving-client
```

Replies contain `ok` and either `value` or `error`. HTTP values contain `status` and `body`. A transport error is distinct from an HTTP refusal; harnesses must inspect both. Request commands supply `host`, `port`, `credential`, `method`, `uri` and an optional string `body` containing the encoded JSON request.

| `op` | Behavior |
| --- | --- |
| `pair` | Supply `link`, `label` and a new `credential` file path. Uses the production direct pairing ceremony; relay links are refused. Creates the credential file with mode 0600 and reports only public identity and endpoints. |
| `request` | Sends one request through the production one-shot transport. `auth` defaults to `paired`; `missing` omits the certificate and `invalid` supplies an unrelated self-signed identity. `body_encoding: hex` returns the exact response bytes as `body_hex` instead of a text `body`. |
| `connect` | Opens a retained TLS carrier using `host`, `port` and `credential`. |
| `held` | Sends a small request on that carrier using a fresh SPL stream ID. |
| `closed` | Waits for EOF or a TLS error on the retained carrier; answers control pings while waiting. Use after revocation to observe actual carrier closure. |
| `disconnect` | Drops the retained carrier. |
| `loss` | Uses a fresh carrier. `loss: before` sends half the declared body without completing the stream. `loss: after` waits for response DATA, then discards it and drops the carrier. |

The held-carrier and fault controls use the SPL framing and assembly library. Their request bodies are limited to 4,096 bytes and responses to 4 MiB. They support bounded synthetic qualification, including a small request that fits the initial flow-control window. The driver does not retry writes. The calling harness must reconcile an uncertain result using the operation's existing contract.

Credential files contain private keys. Keep them outside source control and shared evidence. Receipts may include request bodies and responses, so use synthetic content throughout.
