# 09 Link Serve Prep

Arc: native link-serve, native `sol link serve`.

Scope: ground-truth research only. No implementation decisions recorded here.

## SPL v0.2.0 Source

Line-number citations to spl-rust are pinned to the peeled `v0.2.0^{}`
commit `05bca1c4a4b530ee824c172c57cae7c20a8bb049`. To re-obtain the exact
tree, run `git clone https://github.com/solpbc/spl-rust` and then
`git checkout v0.2.0`.

The local Cargo git db existed at `~/.cargo/git/db/spl-rust-4ddf101ba8563207`,
but cloning it did not expose `v0.2.0`. I cloned from
`https://github.com/solpbc/spl-rust` instead.

Tag verification:

```text
$ git cat-file -t v0.2.0
tag
$ git rev-parse v0.2.0
22dd02eb151f8a4e5c8ce48101f58c23a040205a
$ git rev-parse 'v0.2.0^{}'
05bca1c4a4b530ee824c172c57cae7c20a8bb049
$ git rev-parse HEAD
05bca1c4a4b530ee824c172c57cae7c20a8bb049
```

`v0.2.0` is an annotated tag, so bare `git rev-parse v0.2.0` yields the tag
object `22dd02eb151f8a4e5c8ce48101f58c23a040205a`. The scope's stated commit
`05bca1c4a4b530ee824c172c57cae7c20a8bb049` is correct; it is the peeled
`v0.2.0^{}` commit, and the dereference step was implicit.

## 1. Relay Lane

Result: GAP for native `sol link serve` relay mode from the current Solstone join bundle.

The SPL package can build a relay-capable persistent carrier only when the consumer already has a `Credential` with both `relay_origin` and `device_token`. The current native `sol link join` bundle omits both.

Relevant public seams:

- `TransportClient::new` is public and stores `credential.device_token` in private live state. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/client.rs:74-83`:
  ```rust
  pub fn new(
      credential: Credential,
      token_persist: Option<TokenPersistHook>,
  ) -> Result<Self, TransportError> {
      if credential.relay_origin.is_some() && credential.endpoints.is_empty() {
  ...
      let device_token = credential.device_token.clone().map(tokio::sync::Mutex::new);
  ```
- `TransportClient::dial_carrier` is public and only falls back to relay when `relay_eligible()` is true. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/client.rs:107-147`:
  ```rust
  pub async fn dial_carrier(&self) -> Result<DialedCarrier, TransportError> {
  ...
      if lan_unreachable && self.relay_eligible() {
          let relay = self.dial_carrier_over_relay().await;
          return relay;
      }
  ...
  fn relay_eligible(&self) -> bool {
      self.credential.relay_origin.is_some() && self.device_token.is_some()
  }
  ```
- The persistent relay carrier path requires `relay_origin`, `instance_id`, and current `device_token`, then calls a private relay dialer. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/client.rs:203-236`:
  ```rust
  let origin = self.credential.relay_origin.as_deref().ok_or(TransportError::NoEndpoint)?;
  let instance_id = &self.credential.instance_id;
  let current = self.current_token().await;
  ...
  match dial_relay_carrier(self.config.clone(), origin, instance_id, &token).await {
  ```
- `DialedCarrier` is opaque outside `spl-transport`; consumers cannot construct one directly. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/client.rs:31-45`:
  ```rust
  pub struct DialedCarrier {
      stream: Box<dyn CarrierIo>,
      kind: CarrierKind,
  }
  impl DialedCarrier {
      pub(crate) fn into_parts(self) -> (Box<dyn CarrierIo>, CarrierKind) {
  ```
- The bridge opener trait requires returning that opaque carrier. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:93-107`:
  ```rust
  pub trait CarrierOpener: Send + Sync + 'static {
      fn proxy_headers(...);
      fn dial_carrier(
          &self,
      ) -> Pin<Box<dyn Future<Output = Result<DialedCarrier, TransportError>> + Send + '_>>;
  }
  ```
- `relay::dial_relay_carrier` is not public. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/relay.rs:445-455`:
  ```rust
  pub(crate) struct RelayCarrier {
  ...
  pub(crate) async fn dial_relay_carrier(
      inner_config: Arc<ClientConfig>,
      relay_origin: &str,
      instance_id: &str,
      device_token: &str,
  )
  ```

Other public relay paths are one-shot or require an existing token:

- `relay::dial_relay_ws` is public but requires `device_token` and returns a WebSocket, not `DialedCarrier`. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/relay.rs:322-331`:
  ```rust
  pub async fn dial_relay_ws(
      url: &str,
      device_token: &str,
      outer: Arc<ClientConfig>,
  ) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, TransportError>
  ```
- `relay::request_once_over_ws` is public and sends one HTTP request over an already-established relay WebSocket. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/relay.rs:392-405`:
  ```rust
  pub async fn request_once_over_ws(
      ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
      inner_config: Arc<ClientConfig>,
      handshake_timeout: Duration,
      method: &str,
  ```
- `relay::request_once_relay` is public and one-shot; it also requires `device_token`. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/relay.rs:532-546`:
  ```rust
  pub async fn request_once_relay(
      inner_config: Arc<ClientConfig>,
      relay_origin: &str,
      instance_id: &str,
      device_token: &str,
  ```
- `relay_token::refresh_device_token` is public but requires an existing current token. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/relay_token.rs:34-35`:
  ```rust
  pub async fn refresh_device_token(relay_origin: &str, current_token: &str) -> RefreshOutcome {
  ```
- Direct one-shot `connection::request_once` exists, but it is not a carrier and requires a caller-supplied TLS config and LAN host/port. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/connection.rs:49-67`:
  ```rust
  pub async fn request_once(
      config: Arc<ClientConfig>,
      host: &str,
      port: u16,
  ...
      let tls = dial_tls(config, host, port).await?;
  ```

Public enrollment exists only inside the relay pairing ceremony:

- `pair_over_relay` is public and returns a `Credential` containing relay origin and device token. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/relay_pairing.rs:31-35` and spl-rust v0.2.0 `crates/spl-transport/src/relay_pairing.rs:111-123`:
  ```rust
  pub async fn pair_over_relay(...) -> Result<Credential, TransportError> {
  ...
      relay_origin: Some(link.relay_origin.clone()),
      device_token: Some(device_token),
      device_token_expires_at,
  ```
- The control-plane device enrollment function itself is private. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/relay_pairing.rs:104-127`:
  ```rust
  let device_token =
      enroll_device(&link.relay_origin, &pair.instance_id, home_attestation).await?;
  ...
  async fn enroll_device(
      relay_origin: &str,
      instance_id: &str,
      home_attestation: &str,
  )
  ```

Solstone current bundle persistence:

- The native join bundle file list has no relay origin or device token file. Evidence: `solstone/think/native/link/command.rs:28-34`:
  ```rust
  const BUNDLE_FILES: &[&str] = &[
      "private.pem",
      "cert.pem",
      "chain.pem",
      "home_attestation.jwt",
      "peer.json",
  ];
  ```
- `peer_json` persists identity, label, fingerprint, endpoints, and role only. Evidence: `command.rs:568-595`:
  ```rust
  peer.insert("instance_id".to_string(), Value::String(credential.instance_id.clone()));
  ...
  peer.insert("local_endpoints".to_string(), local_endpoints);
  peer.insert("role".to_string(), Value::String(...));
  ```
- The Solstone seam can receive token fields, so the omission is in the bundle writer. Evidence: `core/crates/solstone-core-sol-client/src/seam.rs:150-161`:
  ```rust
  pub struct LinkJoinCredential {
      ...
      pub relay_device_token: Option<String>,
      pub relay_device_token_expires_at: Option<i64>,
  }
  ```
- The SPL-to-Solstone mapping copies those token fields. Evidence: `core/crates/solstone-core-sol-link/src/lib.rs:57-74`:
  ```rust
  relay_device_token: credential.device_token,
  relay_device_token_expires_at: credential.device_token_expires_at,
  ```

Exact missing public API: a public relay device enrollment or re-enrollment API equivalent to private `enroll_device(relay_origin, instance_id, home_attestation) -> device_token`, and/or a public persistent relay carrier constructor. Without an existing device token, public `dial_relay_ws`, `request_once_relay`, and `TransportClient::dial_carrier` cannot create a resident relay carrier.

Meaning of `--relay-url` on v0.2.0: it can name the relay origin used by public one-shot/persistent relay dials only if the consumer also has a device token. It cannot by itself enroll the device or make the current bundle relay-capable. It therefore cannot provide Python-equivalent relay behavior for native `sol link serve` from the existing bundle.

Acceptance criterion blocked: AC 6 for relay mode. `--direct` can mean LAN-only by omitting relay origin/token from the constructed `Credential`; relay mode is blocked by missing token/origin persistence and missing public first-enrollment API.

## 2. Status Endpoint

Result: GAP for a Python-equivalent local status route.

Only bootstrap is locally answered by the SPL bridge. There is no consumer-supplied local-route hook.

Local-route evidence:

- `CapabilityState::bootstrap_capability` answers only the fixed bootstrap route, and only returns a value when the capability gate is enabled. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:179-185`:
  ```rust
  fn bootstrap_capability(&self, path: &str) -> Option<&str> {
      if path == BOOTSTRAP_ROUTE {
          self.value()
      } else {
          None
      }
  }
  ```
- The request dispatch path checks bootstrap first, then otherwise authorizes and forwards upstream. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:332-414`:
  ```rust
  let bootstrap_capability = runtime.capability.bootstrap_capability(request_head.path());
  let route = if bootstrap_capability.is_some() { "bootstrap" } else { "upstream" };
  ...
  if let Some(capability) = bootstrap_capability {
      handle_bootstrap(...).await;
      return;
  }
  ...
  let upstream_headers = bridge::upstream_request_headers(...);
  if (runtime.stream_response)(&request_head) {
      forward_streaming(...).await;
  } else {
      forward_buffered(...).await;
  }
  ```
- The public config and policy have no local route callback. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:56-69` and spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:111-120`:
  ```rust
  pub struct BridgePolicy {
      pub port: u16,
      pub capability_gate: CapabilityGate,
      pub stream_response: Arc<dyn Fn(&RequestHead) -> bool + Send + Sync>,
      pub request_headers: RequestHeaderPolicy,
      pub max_request_body_bytes: usize,
  }
  ...
  pub struct JournalBridgeConfig {
      pub opener: Arc<dyn CarrierOpener>,
      pub bridge_names: BridgeNames,
      pub endpoint_hosts: Vec<String>,
      pub policy: BridgePolicy,
  }
  ```
- Python serves status before proxying. Evidence: `solstone/think/link/serve_cli.py:239-242`:
  ```python
  if _origin_path(self.path) == STATUS_PATH:
      self._send_status()
      return
  ```

Exact missing public API: a consumer-supplied local-route handler/hook in `journal_bridge` that can inspect a parsed request and return a local HTTP response before upstream authorization/forwarding.

Acceptance criterion blocked: AC 7 for status.

## 3. Status Payload

Python status source:

- Python owns lifecycle fields. Evidence: `solstone/think/link/dialer.py:212-224`:
  ```python
  self._state = STATE_DISCONNECTED
  self._connected_at: float | None = None
  self._last_connected_at: float | None = None
  self._last_failure: TunnelLifecycleFailure | None = None
  self._next_retry_at: float | None = None
  self._reconnect_count = 0
  self._active_requests = 0
  ```
- The nine response keys are returned at `dialer.py:706-724`:
  ```python
  return {
      "health": "healthy" if healthy else "unhealthy",
      "state": state,
      "manager_alive": manager_alive,
      "connected_age_seconds": connected_age,
      "last_connected_at": self._last_connected_at,
      "last_failure": ...,
      "next_retry_at": self._next_retry_at,
      "reconnect_count": self._reconnect_count,
      "active_requests": self._active_requests,
  }
  ```

SPL public handle:

- `JournalBridgeHandle` public API exposes only bound port, contacted flag, bootstrap URL, and shutdown methods. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:123-162`:
  ```rust
  pub struct JournalBridgeHandle {
      port: u16,
      capability: CapabilityState,
      contacted: Arc<AtomicBool>,
      shutdown: oneshot::Sender<()>,
      join: JoinHandle<()>,
  }
  pub fn port(&self) -> u16
  pub fn contacted(&self) -> bool
  pub fn bootstrap_url(&self) -> Option<String>
  pub fn begin_shutdown(self)
  pub async fn shutdown_and_wait(self)
  ```

Private carrier state:

- `MuxCarrier` keeps opener, carrier slot, and keepalive config private and crate-only. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:34-56`:
  ```rust
  pub(crate) struct MuxCarrier {
      opener: Arc<dyn CarrierOpener>,
      slot: Mutex<Option<Arc<CarrierHandle>>>,
      keepalive: KeepaliveConfig,
  }
  ```
- Carrier liveness and stream receivers are crate-private. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:187-197`:
  ```rust
  pub(crate) struct CarrierHandle {
      commands: mpsc::Sender<CarrierCommand>,
      alive: Arc<AtomicBool>,
  }
  pub(crate) struct StreamRx {
      stream_id: u32,
      rx: mpsc::Receiver<StreamEvent>,
      commands: mpsc::Sender<CarrierCommand>,
      cancelled: bool,
  }
  ```
- The active stream map and keepalive counters are local coordinator variables. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:314-323`:
  ```rust
  let mut streams: HashMap<u32, StreamState> = HashMap::new();
  let mut outstanding: Option<OutstandingProbe> = None;
  let mut missed = 0u32;
  ```

Classification:

| Key | Class | Evidence / limit |
| --- | --- | --- |
| `health` | (c) not producible faithfully on v0.2.0 public API | Python health requires `state == connected`, manager alive, session present, and `session.is_alive` (`dialer.py:696-701`). SPL carrier liveness is private in `CarrierHandle.alive` (spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:187-190`). |
| `state` | (b) trackable consumer-side only as a supervisory approximation | A consumer can track its own start/shutdown/error states, but SPL exposes no bridge/carrier state enum; `JournalBridgeHandle` exposes no state getter (spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:131-162`). |
| `manager_alive` | (b) trackable consumer-side only as an approximation | Python checks manager task, loop running, and closed flag (`dialer.py:689-695`). SPL `join` is private inside `JournalBridgeHandle` (spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:123-128`). |
| `connected_age_seconds` | (b) trackable consumer-side after consumer-observed successful `CarrierOpener::dial_carrier` | SPL has no public connected timestamp; consumer can stamp its own successful dial. |
| `last_connected_at` | (b) trackable consumer-side after successful `CarrierOpener::dial_carrier` | Same limit as above. |
| `last_failure` | (b) trackable consumer-side from public `TransportError` returns | SPL logs upstream failures internally, but consumer can record errors returned by its own opener. |
| `next_retry_at` | (b) trackable only if the consumer owns retry/backoff | SPL bridge has retry-on-dead-open behavior in `MuxCarrier::open_stream` but exposes no retry schedule (spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:71-87`). |
| `reconnect_count` | (b) trackable consumer-side by counting dial attempts/reconnect decisions | SPL does not expose `slot` replacement count (spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:103-123`). |
| `active_requests` | (b) trackable only outside the stock handle, by consumer-owned wrapping around local request handling or carrier-open attempts | SPL tracks active streams internally in a private `HashMap` (spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:314-316`) and exposes no request counter through `JournalBridgeHandle`. |

No listed key is derivable directly from the public `JournalBridgeHandle` API.

## 4. `BridgeNames` Values

Struct definition in spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:11-31`:

```rust
pub struct BridgeNames {
    pub capability_cookie_name: String,
    pub upstream_cookie_prefix: String,
    pub observer_header_name: String,
    pub protocol_version_header_name: String,
}
```

Field effects:

- `capability_cookie_name: String`
  - Read during authorization: `check_capability_cookie` uses `head.cookie(&names.capability_cookie_name)`. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:247-255`.
  - Stripped from request cookies before upstream forwarding. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:310-321`:
    ```rust
    if name == names.capability_cookie_name {
        return None;
    }
    ```
  - Used in bootstrap `Set-Cookie`. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:452-455`:
    ```rust
    "Set-Cookie: {}={capability}; {}\r\n"
    ```
- `upstream_cookie_prefix: String`
  - Stripped from local request cookie names before forwarding upstream. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:321-322`:
    ```rust
    let upstream_name = name.strip_prefix(&names.upstream_cookie_prefix)?;
    ```
  - Prepended to upstream response cookie names. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:364-371`:
    ```rust
    format!("{}{}={cookie_value}", names.upstream_cookie_prefix, name.trim())
    ```
- `observer_header_name: String`
  - Rejected as caller auth if present under normalized name. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:106-112`.
  - Stripped from forwarded request headers. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:299-307`.
- `protocol_version_header_name: String`
  - Same rejection and stripping behavior as observer header. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:106-112` and spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:299-307`.

Reserved request header stripping:

```rust
fn is_reserved_request_header(name: &str, names: &BridgeNames) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case(&names.observer_header_name)
        || name.eq_ignore_ascii_case(&names.protocol_version_header_name)
        || name.eq_ignore_ascii_case("host")
        || name.eq_ignore_ascii_case("content-length")
        || HOP_BY_HOP.iter().any(|reserved| name.eq_ignore_ascii_case(reserved))
}
```

Source: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:299-308`.

The observer protocol contract says attribution rides `X-Solstone-Observer` because Python `sol link serve` forwards X headers and strips `Authorization`. Evidence: `solstone/observe/protocol.py:19-23`:

```python
# Resolved before Authorization: Bearer. Survives the `sol link serve` proxy
# (which forwards X-* and strips Authorization), which is why attribution rides
# a new X- header rather than Bearer.
OBSERVER_HANDLE_HEADER = "X-Solstone-Observer"
```

To keep `X-Solstone-Observer` and `X-Solstone-Protocol-Version` flowing upstream, the concrete four `BridgeNames` values must make the two reserved header fields different from those protocol headers. One concrete set that satisfies the code:

```text
capability_cookie_name = "__solstone_link_cap"
upstream_cookie_prefix = ""
observer_header_name = "x-solstone-bridge-observer"
protocol_version_header_name = "x-solstone-bridge-protocol-version"
```

Reasoning: `parse_request_head` normalizes header names to lowercase (spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:60-61`), and `is_reserved_request_header` strips names that match `observer_header_name` or `protocol_version_header_name` case-insensitively (spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:299-307`). If those fields are set to `x-solstone-observer` and `x-solstone-protocol-version`, SPL strips the protocol headers. If they are distinct as above, those two protocol headers are not reserved and can be forwarded under `RequestHeaderPolicy::ForwardAll` or an allow-list that includes them. The cookie fields do not reserve those X headers.

## 5. Response Header and Connection Model Deltas vs Python

### Response-header allow-list vs Python deny-list

SPL response logic:

- Drops `content-length` and hop-by-hop response headers, rewrites cookies and redirects, and otherwise forwards only an allow-list. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:328-352`:
  ```rust
  if name == "content-length" || HOP_BY_HOP.contains(&name.as_str()) {
      continue;
  }
  match name.as_str() {
      "set-cookie" => out.push((name, rewrite_set_cookie(value, names))),
      "location" => out.push((name, rewrite_redirect(...))),
      _ if should_preserve_response_header(&name) => out.push((name, value.clone())),
      _ => {}
  }
  ```
- Allow-list evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:453-479` includes `content-type`, `content-encoding`, `cache-control`, `etag`, `last-modified`, `expires`, `vary`, `accept-ranges`, `content-range`, `www-authenticate`, `retry-after`, CSP/security headers, and cross-origin policy headers.

Python response logic:

- Python only denies seven response hop-by-hop headers. Evidence: `serve_cli.py:48-56`:
  ```python
  RESPONSE_HOP_BY_HOP = {
      "connection",
      "keep-alive",
      "proxy-authenticate",
      "proxy-authorization",
      "te",
      "trailers",
      "upgrade",
  }
  ```
- It forwards everything else from the tunnel response. Evidence: `serve_cli.py:279-284`:
  ```python
  for name, value in first.headers.items():
      if name.lower() in RESPONSE_HOP_BY_HOP:
          continue
      self.send_header(name, value)
  ```

Concrete headers Python forwards that SPL drops:

- `Content-Disposition`: emitted by reflections PDF and news PDF routes. Evidence: `solstone/apps/reflections/routes.py:302-309` and `solstone/apps/news/routes.py:376-384`.
- `X-Solstone-Request-Id`: installed globally on responses. Evidence: `solstone/convey/request_id.py:26-29`.
- `X-Accel-Buffering`: emitted by SSE route. Evidence: `solstone/convey/root.py:257-260`.
- `Transfer-Encoding`: Python response deny-list omits it (`serve_cli.py:48-56`) and test coverage uses it for streaming (`tests/link/test_link_serve.py:167-188`); SPL drops it via `HOP_BY_HOP` in spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:338-339`.
- `Content-Length`: Python forwards it because it is not in the response deny-list; SPL drops upstream `content-length` and writes its own in buffered/HEAD paths. Evidence: SPL drop at spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:338-339` and replacement write at spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:719-724`.
- Arbitrary X headers such as `X-Test`: Python test asserts it forwards `X-Test` (`tests/link/test_link_serve.py:109,133-135`); SPL allow-list would drop it.

### `Set-Cookie`

SPL prefixes upstream cookie names and drops `Domain` and `Secure`. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:359-392`:

```rust
format!("{}{}={cookie_value}", names.upstream_cookie_prefix, name.trim())
...
if attr_name.eq_ignore_ascii_case("domain") || attr_name.eq_ignore_ascii_case("secure") {
    continue;
}
```

Python passes `Set-Cookie` through because it is not in `RESPONSE_HOP_BY_HOP` and the forwarding loop sends all non-denied headers (`serve_cli.py:48-56`, `serve_cli.py:279-284`).

### Request `Host`

SPL strips upstream `host`. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:299-308` includes:

```rust
|| name.eq_ignore_ascii_case("host")
```

Python forwards `Host` because `_forward_request_headers` skips only `Authorization` and request hop-by-hop headers, then adds `Content-Length`. Evidence: `serve_cli.py:408-420`. The behavior is asserted at `tests/link/test_link_serve.py:143-145`:

```python
assert headers["Host"] == f"{serve_cli.LOOPBACK_HOST}:{server.server_address[1]}"
```

### Connection model

SPL handles one request per accepted TCP connection and closes:

- `handle_conn` reads one request and then follows one forwarding path; there is no keep-alive request loop. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:332-414`.
- Streaming path shuts down after stream end. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:617`.
- Buffered path writes `connection: close` and shuts down. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:724-727`.
- Streaming head writes `connection: close`. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:755-757`.

Python uses `ThreadingHTTPServer` / `BaseHTTPRequestHandler`. Evidence: `serve_cli.py:16` and `serve_cli.py:78`. It does not add `Connection: close` on normal proxied responses (`serve_cli.py:279-284`); status/error helpers set `self.close_connection = True` explicitly, for example `serve_cli.py:378-392`.

### Request body and `Transfer-Encoding`

SPL `read_request` reads based on `Content-Length`; absent length becomes body length zero. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:633-675` and spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:677-688`:

```rust
let content_length = parse_content_length(&head)?;
...
Some((head, body))
...
Some(0)
```

Python explicitly rejects request `Transfer-Encoding` before tunnel use. Evidence: `serve_cli.py:311-316`:

```python
if self.headers.get("Transfer-Encoding") is not None:
    self._send_bad_request("Transfer-Encoding is not supported")
    self.close_connection = True
    return None
```

Test evidence: `tests/link/test_link_serve.py:195-218` asserts `400` and no tunnel request.

### `CapabilityGate::Disabled` loopback host

Disabled gate still requires exact loopback host validation. Evidence: spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:373-377`:

```rust
CapabilityState::Disabled => {
    bridge::check_loopback_host(&request_head, runtime.port)
}
```

The only passing value is exactly `127.0.0.1:{port}`. Evidence: spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:201-212`:

```rust
let expected_host = format!("127.0.0.1:{port}");
if head.host() != Some(expected_host.as_str()) {
    return Err(RejectReason::BadHost);
}
```

Therefore a client sending `Host: localhost:5015` to a bridge bound on port `5015` receives a local 403 (spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:379-387`), not upstream forwarding.

## 6. Coverage Arithmetic

Current constants in `scripts/build_native_sol_inventory.py:39-57`:

```python
ENTRY_TYPES = {
    "http",
    "moved-stub",
    "top-level-chat",
    "top-level-import",
    "top-level-link",
    "top-level-notify",
    "local",
}
FINAL_ORACLE_TOTAL = 178
FINAL_HTTP_TOTAL = 152
FINAL_JOURNAL_PYTHON_COMPAT_TOTAL = 23
FINAL_TOP_LEVEL_CHAT_TOTAL = 1
FINAL_TOP_LEVEL_IMPORT_TOTAL = 1
FINAL_TOP_LEVEL_LINK_TOTAL = 1
FINAL_TOP_LEVEL_NOTIFY_TOTAL = 1
FINAL_STUB_COUNTS = {"moved-stub": 2, "local": 1}
```

For a second `top-level-link` entry:

- `FINAL_TOP_LEVEL_LINK_TOTAL` moves from `1` to `2`.
- The `core/native-sol/think/native/link/authority.toml` table gains a second `surface = "sol-link"` / `entry_type = "top-level-link"` entry. Current entry evidence: `authority.toml:7-14`:
  ```toml
  surface = "sol-link"
  path = ["link", "join"]
  operation_id = "link.join"
  entry_type = "top-level-link"
  handler = "link_join"
  ```
- Generated native inventory moves after regeneration.
- Parity vectors must add success/failure bucket coverage for the new top-level link operation.

Constants that do not move for a non-HTTP second top-level-link entry:

- `FINAL_ORACLE_TOTAL`
- `FINAL_HTTP_TOTAL`
- `FINAL_JOURNAL_PYTHON_COMPAT_TOTAL`
- `FINAL_STUB_COUNTS`
- `FINAL_HTTP_GROUP_COUNTS`

Evidence: the complete oracle/HTTP partition filters to `surface == "sol-call"` before counting these values. `scripts/build_native_sol_inventory.py:384-404`:

```python
entries = [entry for entry in entries if entry.surface == "sol-call"]
```

HTTP group counts also count only HTTP entries from that filtered set. Evidence: `build_native_sol_inventory.py:449-481`.

Top-level partition evidence: `build_native_sol_inventory.py:485-506`:

```python
expected = {
    ("sol-chat", "top-level-chat"): FINAL_TOP_LEVEL_CHAT_TOTAL,
    ("sol-import", "top-level-import"): FINAL_TOP_LEVEL_IMPORT_TOTAL,
    ("sol-link", "top-level-link"): FINAL_TOP_LEVEL_LINK_TOTAL,
    ("sol-notify", "top-level-notify"): FINAL_TOP_LEVEL_NOTIFY_TOTAL,
}
```

Coverage script evidence:

- Required top-level link set is collected from `surface == "sol-link"` and `entry_type == "top-level-link"`. Evidence: `scripts/check_native_sol_coverage.py:83-87`.
- Count check uses `FINAL_TOP_LEVEL_LINK_TOTAL`. Evidence: `check_native_sol_coverage.py:163-167`.
- Parity bucket rule for top-level-link. Evidence: `check_native_sol_coverage.py:215-229`:
  ```python
  link_buckets = collect_buckets(
      vectors,
      resolved,
      required_top_level_link,
      {"top-level-link"},
      errors,
  )
  for bucket_name in ("success", "failure"):
      errors.extend(compare_sets(...))
  ```

`link_join.jsonl` satisfies current coverage:

- Current fixture has five vectors, all `surface: "sol-link"`, argv `["link", "join", ...]`, resolving to `link.join`. Evidence: `core/fixtures/native-sol/parity/link_join.jsonl:1-5`.
- Line 1 is a success vector: `expected.exit` is `0`, transport requests is `[]`. `is_success_vector` treats empty request lists as success when exit is 0 because it returns true after iterating zero requests. Evidence: `check_native_sol_coverage.py:429-443`.
- Lines 2-5 are failure vectors because `expected.exit != 0`. Evidence: `check_native_sol_coverage.py:411-413`.

Conformance:

- `scripts/check_native_sol_conformance.py:104` only asserts at least one top-level-link authority exists:
  ```python
  if not any(authority.entry_type == "top-level-link" for authority in authorities):
  ```
- `scripts/check_native_sol_conformance.py:139-140` routes every `top-level-link` to `check_non_http_entry`.
- `check_non_http_entry` requires no HTTP fields and no OpenAPI contract. Evidence: `check_native_sol_conformance.py:204-223`.

Therefore `check_native_sol_conformance.py:104,139` do not need a rule change for a second `top-level-link` entry, unless a new `entry_type` is introduced.

## 7. Baseline

All requested commands were run through `hop check`; no full `make ci` was run.

| Command | Exit | Notes |
| --- | ---: | --- |
| `hop check make check-rust-ios` | 0 | Output included current workspace dependency `spl-core v0.1.0` from tag `v0.1.0`; this stage did not edit `core/Cargo.toml`. |
| `hop check make check-rust-deny` | 0 | Output included duplicate-crate warnings; final line: `bans ok, licenses ok, sources ok`. |
| `hop check make check-rust-fmt` | 0 | `cargo fmt --manifest-path core/Cargo.toml --all -- --check`. |
| `hop check make check-rust-clippy` | 0 | Completed `Finished dev profile`. |
| `hop check make test-only TEST=tests/link/test_link_serve.py` | 0 | 12 passed. |
| `hop check make test-only TEST=tests/sandbox_profile/test_marker.py::test_marker_refusal_matrix_zero_side_effects` | 0 | This named known-failure currently passes on this checkout: 1 passed. |
| `hop check make test-only TEST=tests/test_core_sdist_compile_inputs_integration.py::test_core_sdist_compile_inputs_are_required_by_real_wheel_build` | 2 | Failed at control wheel build: Cargo refused to update `core/Cargo.lock` under `--locked`. |
| `hop check rustup target list --installed` | 0 | Output includes `aarch64-apple-ios`. |
| `hop check cargo deny --version` | 0 | `cargo-deny 0.20.2`. |

Failure excerpt for the second named node:

```text
error: cannot update the lock file .../core/Cargo.lock because --locked was passed to prevent this
assert control.returncode == 0
make: *** [Makefile:492: test-only] Error 1
```

## 8. Python Help Oracle

Derivation command:

```sh
COLUMNS=80 .venv/bin/python - <<'PY'
import argparse
from solstone.think.link.serve_cli import add_arguments
parser = argparse.ArgumentParser(prog='sol link serve')
add_arguments(parser)
text = parser.format_help()
print(len(text.encode()))
print('---')
print(text, end='')
PY
```

Exact byte length: 638.

Exact text:

```text
usage: sol link serve [-h] [--label LABEL] [--port PORT]
                      [--relay-url RELAY_URL] [--direct]

options:
  -h, --help            show this help message and exit
  --label LABEL         Observer link bundle label
  --port PORT           Loopback port to serve on (default: 5015)
  --relay-url RELAY_URL
                        Override the spl relay URL
  --direct              PL-direct only: dial the journal over the LAN secure
                        listener, never the spl relay. Use when the home is
                        reachable directly (same LAN/VPN) to avoid any relay
                        dependency.
```

The scope's 638-byte value is confirmed.

## 9. Latent Defects in `serve_cli.py`

### Post-head `chunks.get()` timeout

Confirmed. The response head wait has a 30-second timeout; post-head reads do not.

Evidence: `serve_cli.py:260-265`:

```python
first = chunks.get(timeout=RESPONSE_HEAD_TIMEOUT_SECONDS)
...
future.cancel()
self._send_gateway_error(TimeoutError("response head timed out"))
```

Evidence: `serve_cli.py:286-288`:

```python
write_body = self.command != "HEAD"
while True:
    item = chunks.get()
```

### `future.cancel()` on running future and bounded queue producer

Confirmed as a defect risk in the Python code path.

Handler cancel sites after the tunnel future has been returned:

- Pre-head invalid/empty response: `serve_cli.py:263`, `271`, `275`.
- Post-head stream exception/unexpected item/write failure: `serve_cli.py:292`, `297`, `307`.

Producer path:

- `proxy_stream_request` submits `_proxy_to_queue` onto the loop and returns a `Future`. Evidence: `dialer.py:488-513`.
- `_proxy_to_queue` keeps writing head/body chunks into the bounded queue, and only handles cancellation if the coroutine receives `asyncio.CancelledError`. Evidence: `dialer.py:559-608`.
- `_put_queue_item` retries forever on `queue.Full`. Evidence: `dialer.py:42-62`:
  ```python
  while True:
      try:
          await asyncio.to_thread(chunks.put, item, True, _QUEUE_PUT_TIMEOUT_SECONDS)
          return
      except queue.Full:
          await asyncio.sleep(0)
  ```

The code has no explicit stop token consumed by `_put_queue_item`; if cancellation does not interrupt the running coroutine while it is repeatedly retrying a full queue, the producer can remain stuck.

### Response hop-by-hop omission

Confirmed. `REQUEST_HOP_BY_HOP` includes `transfer-encoding`, while `RESPONSE_HOP_BY_HOP` omits it.

Evidence: `serve_cli.py:38-56`:

```python
REQUEST_HOP_BY_HOP = {
    ...
    "transfer-encoding",
    "upgrade",
}
RESPONSE_HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "upgrade",
}
```
