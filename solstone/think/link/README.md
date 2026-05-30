# link access helpers

Caller-side link commands and shared pairing helpers.

**Forked from [`github.com/solpbc/spl`](https://github.com/solpbc/spl) `home/` on 2026-04-20.**
The two copies are now fully independent: no pip dep, no submodule, no sync scripts.
The `spl` repo's `home/` continues as the open-source reference implementation of the protocol; this package keeps the caller-side/shared implementation used by pairing, direct dialing, and the link dashboard. The supervised home-side rendezvous daemon lives in `solstone/think/spl/` and runs as `journal spl`.

## layout

| File | Purpose |
|------|---------|
| `cli.py` | Entry point for caller-side `sol link join`, `sol link list`, and `sol link serve`. |
| `serve_cli.py` | Loopback proxy over the PL tunnel for paired caller access. |
| `observer_paths.py` | Shared observer SPL bundle path helpers. |
| `ca.py` | Local CA lifecycle + CSR signing + home-attestation minting. |
| `auth.py` | `authorized_clients.json` reader/writer with mtime-reload and last-seen tracking. |
| `nonces.py` | Pair-ceremony nonce store (shared between CLI and convey pair route). |
| `paths.py` | Journal-path helpers + `SOL_LINK_RELAY_URL` resolution. |

TLS termination, multiplexing, and inline WSGI dispatch now live in
`solstone/convey/secure_listener/`, because Convey owns both listening ports:
the DL web port and the PL secure-listener port 7657.

## naming

- **link** — user-facing and architecturally-visible names: convey app, `sol link`, `sol call link`, `journal/link/`, `/link` route.
- **spl** — the home-side relay daemon (`journal spl`) and protocol-level constructs such as wire-format frames, JWT claim schemas, and reset reason codes. These reference the external stable spl protocol and keep that name.

The home-side daemon still emits Callosum relay-status events on the internal `link` tract so the existing dashboard cache key (`link_connection`) remains stable.

## privacy

No payload bytes are ever logged. The CA private key never leaves `journal/link/ca/private.pem`; service tokens live in `journal/link/tokens/` and device tokens live on paired devices.
