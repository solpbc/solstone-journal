# Mobile journal serving library

`journal-mobile-serving` composes the production Convey router, authenticated
SPL device door and index scanner inside an app-owned worker. The
[Android adapter](../android-runtime/README.md) and
[iOS adapter](../ios-runtime/README.md) supply their private owner controls.

`MobileServing::start(root, address)` retains an admitted journal root and binds
the device door to the supplied IPv4 interface and port. The app creates and
owns that directory. Phone adapters use
`MobileServing::start_with_payload_for_retirement_observed` with the
maintained `core/payload` files packaged by the app. The library validates the
payload contracts and template directories before starting. Setup may withhold the door until the owner completes the
existing mark, lock and finalize flow. `status()` reports the live address.
`request_owner(method, path, body)` dispatches a request admitted by the app's
private IPC; never forward network headers or public IPC into this method.
`index()` runs the production scanner. Drop or `shutdown()` stops the door.

The library opens no unauthenticated loopback listener. Network callers use the
existing TLS identity, pairing, authorization and revocation contracts. Linked
devices are owners; roles remain metadata. A certless pairing peer reaches only
the pairing route during a live consent window. Root binding is checked at
request admission and before scanning; domain writers retain their existing
publication semantics. A changed address requires stopping and starting the
runtime with the new interface address.

The app also starts the maintained agent connections from the agents page:
loopback, local network and your own domain. They use the existing OAuth and
permission checks independently of solstone.me. Stopping the app-owned runtime
closes their listeners and connections. On mobile, the own-domain forwarder
uses the existing TLS loopback ingress, so it does not depend on a Unix socket
under a long app-container path.

Build this standalone development library with:

```sh
cargo check --locked --manifest-path tools/mobile-serving/Cargo.toml
```

Platform packaging and physical qualification belong to the adapters. This
library does not establish background execution, unattended endurance or release
support.

The [synthetic LAN qualification client](client/README.md) uses the same production SPL pairing and request transport. Platform harnesses use its held-carrier and connection-loss controls to check revocation and reconciliation.

With the `processing` feature, authenticated owner requests can discover
read-only linked-share previews at `GET /api/journal/share`. Send
`POST /api/journal/share/match` with `person`, `day` and an optional `facet`,
or `POST /api/journal/share/package` with `person`, `day`, `facet` and
`activity`. Days use `YYYYMMDD`; the other fields use existing journal IDs.
The shared talents return their source references and partial-input warnings.
A preview sends no package to another journal. The service retains its worker
through checked Stop, including a provider call already in progress.

The processing host also runs maintained Think modes through their native
parser and callable owners. Schedule entries retain their existing arguments;
supported Journal actions execute in process. Literal desktop programs and
shell scripts require their desktop host and remain uncompleted on a phone.
The existing `daily_schedule` talent may update `daily_time` from its accepted
result; startup and unrelated work preserve saved schedule and timezone choices.

`POST /api/journal/think` accepts `{ "args": [...] }`, using the native Think
argument grammar. Work requests return `202` with an admitted queue reference;
their normal journal outputs and Health state report the result. Native help
and `--updated` return their output directly. An unavailable foreground queue
returns `503`.

`GET /api/journal/recurrences` reads saved schedules. Use `PUT` or `DELETE` on
`/api/journal/recurrences/think/{name}` for a named Think recurrence. A `PUT`
accepts `args`, `every`, `enabled` and optional `max_runtime`. For the saved
Plaud, Obsidian or audio connection, use
`/api/journal/recurrences/source/{source}` with the same timing fields and no
`args`. These routes create no default recurrence. Apple Health and Oura retain
their existing migration and approval prerequisites.

`POST /api/journal/maintenance/{routine}` submits one admitted maintenance ID,
such as `backup:prune`, to the same foreground queue. It accepts no command,
provider, executable or destination override. Timing edits take effect on the
scheduler's next tick; an already admitted operation keeps its existing
cancellation and completion rules.
