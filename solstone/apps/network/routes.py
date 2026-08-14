# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""network app routes — pair ceremony + paired-device dashboard.

All user-facing work for the spl tunnel integration happens here. The
protocol-level code (TLS, framing, mux) lives in `think/link/`; this
module is the HTTP surface that mobiles and the convey UI hit.

Routes:

  GET  /app/network/            dashboard (paired devices + pair button)
  POST /app/network/pair-start  generate a new nonce + return QR payload
  POST /app/network/pair        mobile posts CSR + nonce; we sign + attest
  POST /app/network/unpair      remove a fingerprint (immediate revocation)
  GET  /app/network/api/devices JSON list of paired devices for JS polling
  GET  /app/network/api/status  service status (for dashboard refresh)

Pair-link QR joins target the secure listener advertised by LINK_DIRECT_PORT
(:7657) and speak its TLS + framed mux protocol before dispatching POST
/app/network/pair into this Flask route. The open nonce admits a cert-less
pairing stream; the QR's CA fingerprint pins the home CA before the signed
client certificate is issued.
"""

from __future__ import annotations

import datetime as dt
import ipaddress
import json as _json
import logging
import re
import socket
from collections.abc import Callable
from dataclasses import asdict, dataclass
from importlib import import_module
from pathlib import Path
from typing import Any

from cryptography.hazmat.primitives import serialization
from flask import Blueprint, Response, abort, current_app, g, jsonify, redirect, request

from solstone.apps.network import copy as link_copy
from solstone.apps.network.copy import (
    PAIR_LINK_HOST,
    PAIR_LINK_PATH,
    link_copy_payload,
)
from solstone.apps.network.crockford32 import encode as crockford_encode
from solstone.apps.network.home_candidates import (
    VPN_SCOPES,
    resolve_pair_link_candidates,
)
from solstone.apps.network.relay_link import (
    derive_rk,
    encode_pair_window_link,
)
from solstone.apps.observer.utils import (
    ObserverRevokeError,
    revoke_observers_bound_to_device,
)
from solstone.apps.utils import log_app_action
from solstone.convey import emit
from solstone.convey.bridge import get_cached_state
from solstone.convey.reasons import (
    CONVEY_OPERATION_FAILED,
    FILE_READ_FAILED,
    INTERNAL_ERROR,
    INVALID_CONFIG_VALUE,
    INVALID_OPERATION_FOR_STATE,
    INVALID_REQUEST_VALUE,
    LOCAL_REQUEST_ONLY,
    MISSING_REQUIRED_FIELD,
    OPERATION_NO_LONGER_AVAILABLE,
    PAIRED_DEVICE_NOT_FOUND,
    PAIRING_KEY_INVALID,
    PAIRING_RELAY_UNAVAILABLE,
    PAIRING_REQUEST_INVALID,
    SERVICE_BUSY,
    SERVICE_OPERATION_FAILED,
)
from solstone.convey.utils import error_response
from solstone.think.link import establish, interface_watcher
from solstone.think.link.auth import AuthorizedClients, ClientEntry, is_peer
from solstone.think.link.ca import (
    generate_nonce,
    generate_pair_window_nonce,
    load_or_generate_ca,
    mint_attestation,
    sign_csr,
)
from solstone.think.link.interface_watcher import get_interface_watcher
from solstone.think.link.link_health import OFFLINE_TUNNEL_REASONS
from solstone.think.link.local_endpoints import (
    LocalEndpoint,
    LocalEndpointsResponse,
    endpoint_to_dict,
    response_to_dict,
)
from solstone.think.link.nonces import Nonce, NonceStore
from solstone.think.link.pair_window import start_pair_window
from solstone.think.link.paths import (
    DEFAULT_RELAY_URL,
    LinkState,
    authorized_clients_path,
    ca_dir,
    load_service_token,
    nonces_path,
    relay_url,
)
from solstone.think.link.window import read_posture
from solstone.think.pairing.config import (
    InvalidHomeAddress,
    clear_home_address,
    get_home_address,
    set_home_address,
    validate_home_address,
)
from solstone.think.services import operations, outcomes, spl, spl_handoff
from solstone.think.services import status as service_status
from solstone.think.utils import get_journal, now_ms

logger = logging.getLogger(__name__)
_SENDER_INSTANCE_ID_RE = re.compile(r"^[A-Za-z0-9-]{1,256}$")
VALID_ROLES = {"", "phone", "observer", "peer"}
_HEALTH_FRESHNESS_MS = 90_000
journal_sources = import_module("solstone.apps.import.journal_sources")
create_state_directory = journal_sources.create_state_directory
load_journal_source_by_fingerprint = journal_sources.load_journal_source_by_fingerprint
save_journal_source = journal_sources.save_journal_source
journal_source_state_prefix = journal_sources.journal_source_state_prefix
mint_pl_journal_source_record = journal_sources.mint_pl_journal_source_record

network_bp = Blueprint(
    "app:network",
    __name__,
    url_prefix="/app/network",
    static_folder="static",
    static_url_path="/static",
)

NATIVE_SOL_ROUTE_REASON_CODES = {
    "api_status": {"pl_revoked"},
    "pair_start": {"pl_revoked"},
    "unpair": {"pl_revoked"},
}


@network_bp.route("/")
def index() -> str:
    # One view object serves both /app/network/ (canonical) and the /app/link/
    # legacy alias. The canonical route serves the SPA shell; the alias index
    # redirects only at the root so every other /app/link/* route keeps resolving
    # through the double-registered blueprint.
    if request.blueprint == "app:link":
        return redirect("/app/network/")
    return current_app.send_static_file("shell.html")


def _authorized() -> AuthorizedClients:
    return AuthorizedClients(authorized_clients_path())


def _nonces() -> NonceStore:
    return NonceStore(nonces_path())


def _utc_now_iso() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _short_fingerprint(fingerprint: str) -> str:
    return fingerprint.removeprefix("sha256:")[:16]


def _default_device_label() -> str:
    now = dt.datetime.now()
    return link_copy.DEVICE_LABEL_DEFAULT_FORMAT.format(
        month=now.strftime("%b"),
        day=now.strftime("%d"),
    )


NETWORK_HOME = "home"


def _rough_network(mode: str) -> str:
    return "anywhere" if mode == "pl-via-spl" else "network"


def _is_hardened_loopback_request() -> bool:
    if request.remote_addr not in {"127.0.0.1", "::1"}:
        return False
    return not any(
        request.headers.get(header)
        for header in ("X-Forwarded-For", "X-Real-IP", "X-Forwarded-Host")
    )


def _read_link_health() -> dict[str, Any] | None:
    health = get_cached_state().get("link_health")
    return health if isinstance(health, dict) else None


def _current_local_endpoints() -> list[LocalEndpoint]:
    watcher = get_interface_watcher()
    return watcher.snapshot() if watcher else []


@dataclass(frozen=True)
class HomeAddressStatus:
    home_address: str | None
    lan_accessible: bool
    candidates: list[str]
    home_candidates: list[dict[str, Any]]
    home_candidates_state: str
    home_candidates_error: str | None


def _resolve_pair_link_candidates(endpoints: list[LocalEndpoint]) -> list[str]:
    """Return resolved direct-pairing IPv4 candidates from local evidence."""

    return resolve_pair_link_candidates(endpoints, _detect_lan_ip())


def _secure_listener_port() -> int:
    """Port the journal advertises in its secure-listener local endpoints.

    Read at call time (monkeypatch-able) and independent of whether the
    interface-watcher snapshot is populated — it can be empty in the CLI/test
    path, so do not read it from _current_local_endpoints().
    """
    return interface_watcher.LINK_DIRECT_PORT


def _home_candidate_entries(
    home_address: str | None,
    candidates: list[str],
) -> list[dict[str, Any]]:
    port = _secure_listener_port()
    detected = [f"{ip}:{port}" for ip in candidates]
    selected = home_address or (detected[0] if detected else None)

    entries: list[dict[str, Any]] = [
        {
            "address": address,
            "selected": address == selected,
            "source": "detected",
        }
        for address in detected
    ]
    if home_address is not None and home_address not in detected:
        entries.append(
            {
                "address": home_address,
                "selected": True,
                "source": "override",
            }
        )
    return entries


def _home_address_host(home_address: str) -> str:
    return home_address.partition(":")[0]


def _home_address_status() -> tuple[HomeAddressStatus, list[LocalEndpoint]]:
    home_address = get_home_address()
    try:
        endpoints = _current_local_endpoints()
        candidates = _resolve_pair_link_candidates(endpoints)
    except Exception:
        logger.exception("link home candidate collection failed")
        if home_address is not None:
            candidates = [_home_address_host(home_address)]
            return (
                HomeAddressStatus(
                    home_address=home_address,
                    lan_accessible=True,
                    candidates=candidates,
                    home_candidates=_home_candidate_entries(home_address, []),
                    home_candidates_state="ready",
                    home_candidates_error=None,
                ),
                [],
            )
        return (
            HomeAddressStatus(
                home_address=None,
                lan_accessible=False,
                candidates=[],
                home_candidates=[],
                home_candidates_state="unavailable",
                home_candidates_error=link_copy.HOME_CANDIDATES_ERROR,
            ),
            [],
        )

    return (
        HomeAddressStatus(
            home_address=home_address,
            lan_accessible=home_address is not None or bool(candidates),
            candidates=candidates,
            home_candidates=_home_candidate_entries(home_address, candidates),
            home_candidates_state="ready",
            home_candidates_error=None,
        ),
        endpoints,
    )


def _detect_lan_ip() -> str | None:
    """Pick a reasonable LAN-facing IPv4 by opening a UDP socket.

    No packets are sent — we just read what src address the kernel would
    pick for a route to an external host. Returns None on any error.
    """
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            sock.connect(("8.8.8.8", 80))
            return sock.getsockname()[0]
        finally:
            sock.close()
    except OSError:
        return None


def _ca_fingerprint() -> str:
    ca = load_or_generate_ca(ca_dir())
    return ca.fingerprint_sha256()


def _build_pair_link(
    host: str,
    port: int,
    nonce: str,
    ca_fp: str,
) -> str:
    """Build the v04 pair-link URL.

    Layout:
    version(1) | addr_type(1) | ipv4(4) | port_be(2) | nonce(16) | ca_fp[:16].
    Encoded as 64-char uppercase Crockford base32 in the URL fragment.
    """
    ipv4_bytes = ipaddress.IPv4Address(host).packed
    port_bytes = port.to_bytes(2, "big")
    nonce_bytes = bytes.fromhex(nonce)
    ca_fp_bytes = bytes.fromhex(ca_fp)[:16]
    blob = b"\x04\x01" + ipv4_bytes + port_bytes + nonce_bytes + ca_fp_bytes
    assert len(blob) == 40
    return f"https://{PAIR_LINK_HOST}{PAIR_LINK_PATH}#{crockford_encode(blob)}"


def _build_pair_link_v05(
    candidates: list[str],
    port: int,
    nonce: str,
    ca_fp: str,
) -> str:
    """Build the v05 multi-address pair-link URL.

    Layout:
    version(1) | addr_type(1) | count(1) | port_be(2) | ipv4(4)*count |
    nonce(16) | ca_fp[:16].

    v05 places the shared port before the address list, unlike v04's single
    address-before-port layout. Count is capped at 4; length is 37 + 4*count.
    """
    count = len(candidates)
    blob = (
        b"\x05\x01"
        + bytes([count])
        + port.to_bytes(2, "big")
        + b"".join(ipaddress.IPv4Address(c).packed for c in candidates)
        + bytes.fromhex(nonce)
        + bytes.fromhex(ca_fp)[:16]
    )
    assert len(blob) == 37 + 4 * count
    return f"https://{PAIR_LINK_HOST}{PAIR_LINK_PATH}#{crockford_encode(blob)}"


@dataclass(frozen=True)
class PairStartResponse:
    nonce: str
    pair_link: str
    expires_in: int
    device_label: str
    ca_fingerprint: str


def _jsonify_preserving_order(payload: dict[str, Any]) -> Response:
    return Response(_json.dumps(payload), mimetype="application/json")


def _derive_relay_state(token_present: bool) -> str:
    """Return pre-mechanism relay attachment state.

    connecting/parked are valid contract values but are not produced until
    parking is wired.
    """
    return "offline" if token_present else "not-enrolled"


def _link_health_is_fresh(health: dict[str, Any], now_ms_val: int) -> bool:
    ts = health.get("ts")
    return isinstance(ts, int) and now_ms_val - ts <= _HEALTH_FRESHNESS_MS


def _current_tunnel_error(health: dict[str, Any]) -> str | None:
    error = health.get("last_relay_tunnel_error")
    if not isinstance(error, str):
        return None
    error_at = health.get("last_relay_tunnel_error_at") or 0
    success_at = health.get("last_successful_relay_tunnel_at") or 0
    return error if error_at >= success_at else None


def _derive_spl_relay_state(
    token_present: bool,
    health: dict[str, Any] | None,
    now_ms_val: int,
) -> str:
    if not token_present:
        return "not-enrolled"
    if health is None:
        return "connecting"
    if not _link_health_is_fresh(health, now_ms_val):
        return "offline"
    current_error = _current_tunnel_error(health)
    if current_error in OFFLINE_TUNNEL_REASONS:
        return "offline"
    state = health.get("state")
    if state == "reconnecting":
        return "reconnecting"
    if state == "connected":
        return "parked"
    return "connecting"


def _derive_reachability(
    lan_accessible: bool,
    posture: str,
    relay_state: str,
) -> str:
    if not lan_accessible:
        return "lan-unreachable"
    if posture == "direct":
        return "online"
    # posture == "spl": map relay_state.
    return {
        "connecting": "finishing-setup",
        "parked": "online",
        "reconnecting": "reconnecting",
        "offline": "offline",
        "not-enrolled": "finishing-setup",
    }[relay_state]


def _private_link_status() -> dict[str, Any]:
    resting = service_status.spl_status()
    state = str(resting["state"])
    return {
        "service": "spl",
        "state": state,
        "posture": read_posture(),
        "enrolled": load_service_token() is not None,
        "relay_url": relay_url(),
        "actions": {
            "enable": state in {"not_enabled", "inconsistent"},
            "disable": state in {"enabled", "inconsistent"},
        },
        "operation": operations.operation_for_service("spl"),
    }


def _start_operation_response(
    service: str,
    kind: str,
    portal_url: str | None,
    flow: Callable[[], operations.HandoffResult],
) -> tuple[Response, int]:
    try:
        operation = operations.start_operation(service, kind, portal_url, flow)
    except operations.OperationBusyError:
        return error_response(SERVICE_BUSY, detail="operation already running")
    return jsonify({"success": True, "service": service, "operation": operation}), 202


# ---------------------------------------------------------------------------
# dashboard
# ---------------------------------------------------------------------------


@network_bp.route("/api/state")
def api_state() -> Any:
    try:
        return jsonify({"posture": read_posture(), "link_copy": link_copy_payload()})
    except Exception:
        logger.exception("network state load failed")
        return error_response(
            FILE_READ_FAILED,
            detail="Failed to load network state.",
        )


@network_bp.route("/api/devices")
def api_devices() -> Any:
    """JSON list of paired devices — used by the dashboard JS."""
    entries = _authorized().snapshot()
    devices = [_entry_to_json(e) for e in entries]
    return jsonify({"devices": devices})


@network_bp.route("/api/status")
def api_status() -> Any:
    """Snapshot of link-service state for the dashboard header."""
    now_ms_val = now_ms()
    health = _read_link_health()
    state = LinkState.load()
    token = load_service_token()
    token_present = token is not None
    ca_fp = _ca_fingerprint() if ca_dir().exists() else None
    home_status, local_endpoints = _home_address_status()
    posture = read_posture()
    relay_state = (
        _derive_spl_relay_state(token_present, health, now_ms_val)
        if posture == "spl"
        else _derive_relay_state(token_present)
    )
    reachability = _derive_reachability(
        home_status.lan_accessible,
        posture,
        relay_state,
    )
    vpn_candidates = [
        {"label": ep.scope, "address": f"{ep.ip}:{ep.port}"}
        for ep in local_endpoints
        if ep.scope in VPN_SCOPES
    ]
    return jsonify(
        {
            "instance_id": state.instance_id if state else None,
            "home_label": state.home_label if state else None,
            "enrolled": token_present,
            "relay_url": relay_url(),
            "ca_fingerprint": ca_fp,
            "lan_accessible": home_status.lan_accessible,
            "posture": posture,
            "reachability": reachability,
            "relay_state": relay_state,
            "last_link_event_at": health["ts"] if health else None,
            "relay_listen_generation": health["listen_generation"] if health else None,
            "last_successful_relay_tunnel_at": (
                health["last_successful_relay_tunnel_at"] if health else None
            ),
            "last_relay_tunnel_error": (
                health["last_relay_tunnel_error"] if health else None
            ),
            "last_relay_tunnel_error_at": (
                health["last_relay_tunnel_error_at"] if health else None
            ),
            "last_relay_listener_ack_at": (
                health["last_relay_listener_ack_at"] if health else None
            ),
            "last_relay_listener_ack_generation": (
                health["last_relay_listener_ack_generation"] if health else None
            ),
            "home_address": home_status.home_address,
            "vpn": {"active": None, "candidates": vpn_candidates},
            "home_candidates": home_status.home_candidates,
            "home_candidates_state": home_status.home_candidates_state,
            "home_candidates_error": home_status.home_candidates_error,
        }
    )


@network_bp.route("/api/identity")
def api_identity() -> Any:
    """Read-only committed journal mark + id for the identity header.

    Display-only. Derives the immutable mark once per request (argon2id is
    deliberately costly — never put this on a poll). Any failure degrades to a
    neutral not-committed payload at HTTP 200; the exception is logged but the
    client never sees a 500.
    """
    neutral = {"committed": False, "instance_id": None, "mark": None}
    try:
        if not establish.is_committed():
            return jsonify(neutral)
        state = LinkState.load()
        if state is None:
            return jsonify(neutral)
        mark = establish.committed_mark()
        return jsonify(
            {
                "committed": True,
                "instance_id": state.instance_id,
                "mark": mark.to_render_spec(),
            }
        )
    except Exception:
        logger.exception("network identity derivation failed")
        return jsonify(neutral)


@network_bp.route("/api/private-link")
def api_private_link() -> Any:
    return jsonify({"success": True, **_private_link_status()})


@network_bp.route("/private-link/enable", methods=["POST"])
def private_link_enable() -> tuple[Response, int]:
    if _private_link_status()["state"] == "enabled":
        return error_response(
            INVALID_OPERATION_FOR_STATE,
            detail=outcomes.SPL_PRIVATE_LINK_ALREADY_ENABLED_DETAIL,
        )
    try:
        consent_url, nonce, base_url = spl_handoff.build_spl_handoff_url()
    except OSError:
        return error_response(
            SERVICE_OPERATION_FAILED,
            detail=outcomes.SPL_PRIVATE_LINK_CONSENT_LINK_PREPARE_FAILED_DETAIL,
        )
    return _start_operation_response(
        "spl",
        "spl_enable",
        consent_url,
        lambda: spl_handoff.run_spl_handoff(nonce=nonce, base_url=base_url),
    )


@network_bp.route("/private-link/disable", methods=["POST"])
def private_link_disable() -> tuple[Response, int]:
    try:
        outcome = spl.disable_spl()
    except Exception:
        logger.exception("link private-link disable failed")
        return error_response(SERVICE_OPERATION_FAILED)
    return (
        jsonify(
            {
                "success": True,
                "service": "spl",
                "result": {"was_enabled": outcome.was_enabled},
                "status": _private_link_status(),
            }
        ),
        200,
    )


@network_bp.route("/host-address", methods=["POST"])
def set_home_address_route() -> Any:
    payload = request.get_json(silent=True) or {}
    if not isinstance(payload, dict):
        payload = {}
    raw_address = payload.get("home_address")
    home_address = raw_address if isinstance(raw_address, str) else None
    try:
        if home_address is not None and home_address.strip():
            set_home_address(validate_home_address(home_address))
        else:
            clear_home_address()
    except InvalidHomeAddress as exc:
        return error_response(INVALID_CONFIG_VALUE, detail=str(exc))
    home_status, _ = _home_address_status()
    return jsonify({"ok": True, "home_address": home_status.home_address})


@network_bp.get("/local-endpoints")
def local_endpoints() -> Any:
    if not _is_hardened_loopback_request():
        abort(404)
    response = LocalEndpointsResponse(
        v=1,
        endpoints=tuple(_current_local_endpoints()),
        ttl_s=3600,
        generated_at=_utc_now_iso(),
    )
    return jsonify(response_to_dict(response))


def _same_machine_requested(payload: dict[str, Any]) -> bool | None:
    value = payload.get("same_machine")
    if value is None:
        return False
    if isinstance(value, bool):
        return value
    return None


# ---------------------------------------------------------------------------
# pair ceremony
# ---------------------------------------------------------------------------


@network_bp.route("/api/pair/nonce-status")
def api_pair_nonce_status() -> Any:
    entry = _nonces().peek(request.args.get("nonce", ""))
    return jsonify({"present": entry is not None, "used": bool(entry and entry.used)})


@network_bp.route("/pair-start", methods=["POST"])
def pair_start() -> Any:
    """Generate a single-use 5-minute nonce and return link-ready payload."""
    payload = request.get_json(silent=True) or {}
    device_label = str(payload.get("device_label") or "").strip()
    raw_role = payload.get("role", "")
    role = "" if raw_role is None else raw_role
    if not isinstance(role, str) or role not in VALID_ROLES:
        return error_response(PAIRING_REQUEST_INVALID, detail="invalid role")

    same_machine = _same_machine_requested(payload)
    if same_machine is None:
        return error_response(
            PAIRING_REQUEST_INVALID,
            detail="same_machine must be boolean",
        )
    if same_machine:
        if not _is_hardened_loopback_request():
            return error_response(LOCAL_REQUEST_ONLY)
        ca_fp = _ca_fingerprint()
        port = _secure_listener_port()
        nonce = generate_nonce()
        pair_link = _build_pair_link("127.0.0.1", port, nonce, ca_fp)
        _nonces().add(
            nonce,
            device_label,
            role=role,
            same_machine=True,
        )
        response = PairStartResponse(
            nonce=nonce,
            pair_link=pair_link,
            expires_in=300,
            device_label=device_label,
            ca_fingerprint=ca_fp,
        )
        return _jsonify_preserving_order(asdict(response))

    if read_posture() == "spl":
        service_token = load_service_token()
        if service_token is None:
            return error_response(
                INVALID_OPERATION_FOR_STATE,
                detail="spl posture requires a relay service token; none is configured",
            )

        ca = load_or_generate_ca(ca_dir())
        ca_fp = ca.fingerprint_sha256()
        s = generate_pair_window_nonce()
        origin = relay_url()
        relay_origin = None if origin == DEFAULT_RELAY_URL else origin
        pair_link = encode_pair_window_link(
            s,
            ca.spki_fingerprint_sha256(),
            relay_origin=relay_origin,
        )
        nonce = s.hex()
        handle = start_pair_window(
            rk=derive_rk(s),
            service_token=service_token,
            relay_endpoint=origin,
        )
        if not handle.wait_open():
            handle.cancel()
            return error_response(
                PAIRING_RELAY_UNAVAILABLE,
                detail="the pairing window couldn't be opened with the relay; try again",
            )
    else:
        ca_fp = _ca_fingerprint()
        port = _secure_listener_port()
        home_address = get_home_address()
        if home_address is not None:
            candidates = [_home_address_host(home_address)]
        else:
            try:
                candidates = _resolve_pair_link_candidates(_current_local_endpoints())
            except Exception:
                logger.exception("link pair-start candidate collection failed")
                candidates = []
        if not candidates:
            return error_response(
                PAIRING_REQUEST_INVALID,
                detail="pair-link requires an IPv4 LAN address; none found",
            )
        nonce = generate_nonce()
        if len(candidates) == 1:
            pair_link = _build_pair_link(candidates[0], port, nonce, ca_fp)
        else:
            pair_link = _build_pair_link_v05(candidates, port, nonce, ca_fp)

    _nonces().add(
        nonce,
        device_label,
        role=role,
    )
    response = PairStartResponse(
        nonce=nonce,
        pair_link=pair_link,
        expires_in=300,
        device_label=device_label,
        ca_fingerprint=ca_fp,
    )
    return _jsonify_preserving_order(asdict(response))


def _complete_pairing(
    consumed: Nonce,
    csr_pem: str,
    assigned_label: str,
    client_label: str,
    *,
    network: str,
    sender_instance_id: str | None = None,
) -> tuple[dict[str, Any], ClientEntry, str]:
    ca = load_or_generate_ca(ca_dir())
    cert_label = client_label or assigned_label or _default_device_label()
    client_cert_pem, fingerprint = sign_csr(ca, csr_pem, cert_label)

    state = LinkState.load_or_create()
    paired_at = _utc_now_iso()
    attestation = mint_attestation(ca, state.instance_id, fingerprint)
    ca_chain_pem = ca.cert.public_bytes(serialization.Encoding.PEM).decode("ascii")
    response: dict[str, Any] = {
        "client_cert": client_cert_pem,
        "ca_chain": [ca_chain_pem],
        "instance_id": state.instance_id,
        "home_label": state.home_label,
        "home_attestation": attestation,
        "fingerprint": fingerprint,
    }
    endpoints = _current_local_endpoints()
    if endpoints:
        response["local_endpoints"] = [endpoint_to_dict(ep) for ep in endpoints]

    journal_source_record_path = None
    try:
        if is_peer(consumed.role):
            journal_source_record_path = mint_pl_journal_source_record(
                fingerprint=fingerprint,
                device_label=cert_label,
                paired_at=paired_at,
                peer_instance_id=sender_instance_id,
            )
            create_state_directory(Path(get_journal()), journal_source_record_path.stem)
        entry = _authorized().add(
            fingerprint=fingerprint,
            device_label=assigned_label,
            instance_id=state.instance_id,
            role="peer" if is_peer(consumed.role) else "",
            paired_at=paired_at,
            network=network,
            client_label=client_label,
        )
    except Exception:
        if journal_source_record_path is not None:
            try:
                journal_source_record_path.unlink()
            except FileNotFoundError:
                pass
        raise

    return response, entry, paired_at


def _emit_pair_complete(
    device_label: str,
    fingerprint: str,
    paired_at: str,
    *,
    network: str,
) -> None:
    emit(
        "link",
        "pair_complete",
        device_label=device_label,
        fingerprint=fingerprint,
        fingerprint_short=_short_fingerprint(fingerprint),
        paired_at=paired_at,
        network=network,
    )


@network_bp.route("/pair", methods=["POST"])
def pair() -> Any:
    """Mobile pair endpoint — accepts CSR + nonce, signs + mints attestation.

    Query: `?token=<nonce>` (the nonce minted by /pair-start).
    Body  (JSON):
        {
          "csr":          "<PEM>",      // required
          "device_label": "<string>",   // optional client self-name
          "nonce":        "<hex>"       // optional: may be in body instead of query
        }

    Response on success (200):
        {
          "client_cert":       "<PEM>",
          "ca_chain":          ["<PEM>", ...],
          "instance_id":       "<uuid>",
          "home_label":        "<string>",
          "home_attestation":  "<JWT>",
          "fingerprint":       "sha256:<hex>"
        }
    """
    body = request.get_json(silent=True) or {}
    nonce_value = request.args.get("token") or body.get("nonce")
    csr_pem = body.get("csr")
    device_label = str(body.get("device_label") or "").strip()

    if not isinstance(nonce_value, str) or not isinstance(csr_pem, str):
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="missing fields (nonce + csr required)",
        )
    raw_sender_instance_id = body.get("sender_instance_id")
    sender_instance_id: str | None = None
    if raw_sender_instance_id is not None:
        if not isinstance(
            raw_sender_instance_id, str
        ) or not _SENDER_INSTANCE_ID_RE.fullmatch(raw_sender_instance_id):
            return error_response(
                PAIRING_REQUEST_INVALID,
                detail=f"bad sender_instance_id: {raw_sender_instance_id}",
            )
        sender_instance_id = raw_sender_instance_id

    consumed = _nonces().consume(nonce_value)
    if consumed is None:
        return error_response(
            OPERATION_NO_LONGER_AVAILABLE,
            detail="nonce expired or used",
        )

    assigned_label = consumed.device_label
    client_label = device_label

    # A machine pairing with itself reaches the journal over loopback, which the
    # listener reports the same way it reports a relay tunnel. Reach derived from
    # the peer would therefore read as `anywhere` for the owner's own machine.
    network = NETWORK_HOME if consumed.same_machine else _rough_network(g.identity.mode)
    try:
        response, entry, paired_at = _complete_pairing(
            consumed,
            csr_pem,
            assigned_label,
            client_label,
            network=network,
            sender_instance_id=sender_instance_id,
        )
    except ValueError as exc:
        logger.info("pair: bad csr: %s", exc)
        return error_response(PAIRING_KEY_INVALID, detail=f"bad csr: {exc}")
    _emit_pair_complete(
        entry.display_label,
        entry.fingerprint,
        paired_at,
        network=network,
    )
    return jsonify(response)


def _revoked_observer_projection(observers: list[dict]) -> list[dict[str, str]]:
    projected = [
        {
            "name": str(observer.get("name") or ""),
            "prefix": str(observer.get("filename_prefix") or ""),
        }
        for observer in observers
    ]
    return sorted(projected, key=lambda item: (item["name"], item["prefix"]))


@network_bp.route("/rename", methods=["POST"])
def rename() -> Any:
    """Rename a paired device by fingerprint."""
    body = request.get_json(silent=True) or {}
    fingerprint = body.get("fingerprint")
    label = body.get("label")
    if not isinstance(fingerprint, str) or not fingerprint.strip():
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="fingerprint and label required",
        )
    if not isinstance(label, str):
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="fingerprint and label required",
        )

    authorized = _authorized()
    try:
        updated = authorized.update_label(fingerprint.strip(), label)
    except ValueError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except OSError as exc:
        logger.error("rename: failed to persist label for %s: %s", fingerprint, exc)
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="couldn't save the new label",
        )
    if updated is None:
        return error_response(PAIRED_DEVICE_NOT_FOUND, detail="fingerprint not paired")
    return jsonify(
        {
            "fingerprint": updated.fingerprint,
            "device_label": updated.device_label,
            "display_label": updated.display_label,
        }
    )


def _ambiguous_unpair_label_detail(label: str, entries: list[ClientEntry]) -> str:
    sorted_entries = sorted(
        entries, key=lambda entry: (entry.paired_at, entry.fingerprint)
    )
    lines = [link_copy.UNPAIR_AMBIGUOUS_LABEL_HEADER_FORMAT.format(label=label)]
    for entry in sorted_entries:
        lines.append(
            link_copy.UNPAIR_AMBIGUOUS_LABEL_CANDIDATE_FORMAT.format(
                paired_at=entry.paired_at,
                short_fp=_short_fingerprint(entry.fingerprint),
            )
        )
        lines.append(
            link_copy.UNPAIR_AMBIGUOUS_LABEL_COMMAND_FORMAT.format(
                fingerprint=entry.fingerprint
            )
        )
    return "\n".join(lines)


@network_bp.route("/unpair", methods=["POST"])
def unpair() -> Any:
    """Revoke a paired device by label or fingerprint.

    Body (JSON): {"fingerprint": "sha256:..."} or {"device_label": "..."}
    """
    body = request.get_json(silent=True) or {}
    raw_fingerprint = body.get("fingerprint")
    raw_device_label = body.get("device_label")
    fingerprint = raw_fingerprint.strip() if isinstance(raw_fingerprint, str) else None
    device_label = (
        raw_device_label.strip() if isinstance(raw_device_label, str) else None
    )
    fingerprint = fingerprint or None
    device_label = device_label or None

    authorized = _authorized()
    entry: ClientEntry | None = None
    if fingerprint is not None:
        entry = authorized.get(fingerprint)
    elif device_label is not None:
        matches = authorized.find_all_by_display_label(device_label)
        if len(matches) == 1:
            entry = matches[0]
            fingerprint = entry.fingerprint
        elif matches:
            return error_response(
                INVALID_OPERATION_FOR_STATE,
                detail=_ambiguous_unpair_label_detail(device_label, matches),
            )
    else:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="fingerprint or device_label required",
        )

    if entry is None:
        detail = (
            "fingerprint not paired"
            if fingerprint is not None
            else "no paired device with that label"
        )
        return error_response(
            PAIRED_DEVICE_NOT_FOUND,
            detail=detail,
        )

    short_fp = _short_fingerprint(fingerprint)
    role = entry.role

    if is_peer(role):
        source = load_journal_source_by_fingerprint(fingerprint)
        if source is None:
            logger.warning("unpair: peer journal source missing for %s", short_fp)
        elif source.get("revoked"):
            logger.warning("unpair: peer journal source %s already revoked", short_fp)
        else:
            source["revoked"] = True
            source["revoked_at"] = now_ms()
            if save_journal_source(source):
                log_app_action(
                    app="import",
                    facet=None,
                    action="journal_source_revoke",
                    params={
                        "name": source.get("device_label") or source.get("name"),
                        "key_prefix": journal_source_state_prefix(source),
                    },
                )
            else:
                logger.error(
                    "unpair: failed to save peer journal source for %s", short_fp
                )
        authorized.remove(fingerprint)
    else:
        authorized.remove(fingerprint)
    try:
        revoked_observers = revoke_observers_bound_to_device(fingerprint)
    except ObserverRevokeError as exc:
        return error_response(
            INTERNAL_ERROR,
            detail="Failed to revoke one or more bound observer streams.",
            extra={
                "unpaired": fingerprint,
                "revoked_observers": _revoked_observer_projection(exc.revoked),
                "failed_operation": "observer_revoke",
            },
        )
    return jsonify(
        {
            "unpaired": fingerprint,
            "revoked_observers": _revoked_observer_projection(revoked_observers),
        }
    )


def _entry_to_json(entry: ClientEntry) -> dict[str, Any]:
    short_fp = _short_fingerprint(entry.fingerprint)
    return {
        "fingerprint": entry.fingerprint,
        "fingerprint_short": short_fp,
        "device_label": entry.device_label,
        "display_label": entry.display_label,
        "client_label": entry.client_label,
        "paired_at": entry.paired_at,
        "last_seen_at": entry.last_seen_at,
        "role": entry.role,
        "network": entry.network,
        "kind": entry.kind,
        "observer_handle": entry.observer_handle,
    }
