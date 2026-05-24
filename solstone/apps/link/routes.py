# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""link app routes — pair ceremony + paired-device dashboard.

All user-facing work for the spl tunnel integration happens here. The
protocol-level code (TLS, framing, mux) lives in `think/link/`; this
module is the HTTP surface that mobiles and the convey UI hit.

Routes:

  GET  /link                    dashboard (paired devices + pair button)
  GET  /link/qr.png             QR image for an active nonce (via ?token=)
  POST /link/pair-start         generate a new nonce + return QR payload
  POST /link/pair               mobile posts CSR + nonce; we sign + attest
  POST /link/unpair             remove a fingerprint (immediate revocation)
  GET  /link/api/devices        JSON list of paired devices for JS polling
  GET  /link/api/status         service status (for dashboard refresh)

The pair hop is plain HTTP on convey's existing listener — there is no
separate port. Integrity is provided by the CA-fingerprint pinned in the
QR, not by transport TLS. A MITM on the LAN can observe the nonce but
cannot forge a cert signed by the pinned CA.
"""

from __future__ import annotations

import datetime as dt
import ipaddress
import json as _json
import logging
import re
import socket
from dataclasses import asdict, dataclass
from typing import Any

from cryptography.hazmat.primitives import serialization
from flask import Blueprint, Response, abort, jsonify, request

from solstone.apps.link import copy as link_copy
from solstone.apps.link.copy import (
    MANUAL_CODE_LEN,
    PAIR_LINK_HOST,
    PAIR_LINK_PATH,
)
from solstone.apps.link.crockford32 import encode as crockford_encode
from solstone.apps.link.manual_code import (
    generate as generate_manual_code,
)
from solstone.apps.link.manual_code import (
    normalize as normalize_manual_code,
)
from solstone.convey import emit
from solstone.convey.reasons import (
    MISSING_REQUIRED_FIELD,
    OPERATION_NO_LONGER_AVAILABLE,
    PAIRED_DEVICE_NOT_FOUND,
    PAIRING_KEY_INVALID,
    PAIRING_REQUEST_INVALID,
)
from solstone.convey.utils import error_response
from solstone.think.link.auth import AuthorizedClients, ClientEntry
from solstone.think.link.ca import (
    generate_nonce,
    load_or_generate_ca,
    mint_attestation,
    sign_csr,
)
from solstone.think.link.interface_watcher import get_interface_watcher
from solstone.think.link.local_endpoints import (
    LocalEndpoint,
    LocalEndpointsResponse,
    endpoint_to_dict,
    response_to_dict,
)
from solstone.think.link.nonces import Nonce, NonceStore
from solstone.think.link.paths import (
    LinkState,
    authorized_clients_path,
    ca_dir,
    load_account_token,
    nonces_path,
    relay_url,
)

logger = logging.getLogger(__name__)
MANUAL_CODE_RE = re.compile(rf"^[0-9A-HJKMNP-TV-Z]{{{MANUAL_CODE_LEN}}}$")
VALID_ROLES = {"phone", "observer", "peer"}

link_bp = Blueprint(
    "app:link",
    __name__,
    url_prefix="/app/link",
)


def _authorized() -> AuthorizedClients:
    return AuthorizedClients(authorized_clients_path())


def _nonces() -> NonceStore:
    return NonceStore(nonces_path())


def _utc_now_iso() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _default_device_label() -> str:
    now = dt.datetime.now()
    return link_copy.DEVICE_LABEL_DEFAULT_FORMAT.format(
        month=now.strftime("%b"),
        day=now.strftime("%d"),
    )


def _is_loopback_request() -> bool:
    return request.remote_addr in {"127.0.0.1", "::1"}


def _current_local_endpoints() -> list[LocalEndpoint]:
    watcher = get_interface_watcher()
    return watcher.snapshot() if watcher else []


def _resolve_host_port() -> str:
    """Best-effort LAN host:port for the convey host."""
    host = request.host
    try:
        hostname, _, port = host.partition(":")
        if hostname in ("localhost", "127.0.0.1", "::1", "0.0.0.0"):
            lan_ip = _detect_lan_ip()
            if lan_ip:
                host = f"{lan_ip}:{port}" if port else lan_ip
    except Exception:
        logger.debug("lan ip detection failed", exc_info=True)
    return host


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
    """Build the v2 pair-link URL.

    Layout:
    version(1) | addr_type(1) | ipv4(4) | port_be(2) | nonce(8) | ca_fp[:16].
    Encoded as 52-char uppercase Crockford base32 in the URL fragment.
    """
    ipv4_bytes = ipaddress.IPv4Address(host).packed
    port_bytes = port.to_bytes(2, "big")
    nonce_bytes = bytes.fromhex(nonce)
    ca_fp_bytes = bytes.fromhex(ca_fp)[:16]
    blob = b"\x02\x01" + ipv4_bytes + port_bytes + nonce_bytes + ca_fp_bytes
    assert len(blob) == 32
    return f"https://{PAIR_LINK_HOST}{PAIR_LINK_PATH}#{crockford_encode(blob)}"


@dataclass(frozen=True)
class PairStartResponse:
    nonce: str
    pair_link: str
    manual_code: str
    expires_in: int
    device_label: str
    lan_url: str
    ca_fingerprint: str


def _jsonify_preserving_order(payload: dict[str, Any]) -> Response:
    return Response(_json.dumps(payload), mimetype="application/json")


def _is_lan_accessible() -> bool:
    """Check whether convey is bound to a non-loopback interface.

    Used to drive the "enable LAN access" nudge on /link. Best-effort: the
    signal is the Host header the dashboard loaded under.
    """
    hostname, _, _ = request.host.partition(":")
    if hostname in ("localhost", "127.0.0.1", "::1"):
        return bool(_detect_lan_ip())
    return True


# ---------------------------------------------------------------------------
# dashboard
# ---------------------------------------------------------------------------


@link_bp.route("/api/devices")
def api_devices() -> Any:
    """JSON list of paired devices — used by the dashboard JS."""
    entries = _authorized().snapshot()
    devices = [_entry_to_json(e) for e in entries]
    return jsonify({"devices": devices})


@link_bp.route("/api/status")
def api_status() -> Any:
    """Snapshot of link-service state for the dashboard header."""
    state = LinkState.load_or_create()
    token = load_account_token()
    ca_fp = _ca_fingerprint() if ca_dir().exists() else None
    return jsonify(
        {
            "instance_id": state.instance_id,
            "home_label": state.home_label,
            "enrolled": token is not None,
            "relay_url": relay_url(),
            "ca_fingerprint": ca_fp,
            "lan_accessible": _is_lan_accessible(),
        }
    )


@link_bp.get("/local-endpoints")
def local_endpoints() -> Any:
    if not _is_loopback_request():
        abort(404)
    response = LocalEndpointsResponse(
        v=1,
        endpoints=tuple(_current_local_endpoints()),
        ttl_s=3600,
        generated_at=_utc_now_iso(),
    )
    return jsonify(response_to_dict(response))


# ---------------------------------------------------------------------------
# pair ceremony
# ---------------------------------------------------------------------------


@link_bp.route("/pair-start", methods=["POST"])
def pair_start() -> Any:
    """Generate a single-use 5-minute nonce and return link-ready payload."""
    payload = request.get_json(silent=True) or {}
    device_label = (
        str(payload.get("device_label") or "").strip() or _default_device_label()
    )
    role = payload.get("role", "phone")
    if not isinstance(role, str) or role not in VALID_ROLES:
        return error_response(PAIRING_REQUEST_INVALID, detail="invalid role")

    lan_url = _resolve_host_port()
    hostname, _, port_str = lan_url.partition(":")
    try:
        ipaddress.IPv4Address(hostname)
    except ValueError:
        return error_response(
            PAIRING_REQUEST_INVALID,
            detail=f"pair-link requires an IPv4 LAN address; got {hostname!r}",
        )
    port = int(port_str) if port_str else 80

    ca_fp = _ca_fingerprint()
    nonce = generate_nonce()
    manual_code_hyphenated = generate_manual_code()
    pair_link = _build_pair_link(hostname, port, nonce, ca_fp)
    _nonces().add(
        nonce,
        device_label,
        role=role,
        manual_code=normalize_manual_code(manual_code_hyphenated),
    )
    response = PairStartResponse(
        nonce=nonce,
        pair_link=pair_link,
        manual_code=manual_code_hyphenated,
        expires_in=300,
        device_label=device_label,
        lan_url=lan_url,
        ca_fingerprint=ca_fp,
    )
    return _jsonify_preserving_order(asdict(response))


def _complete_pairing(
    consumed: Nonce,
    csr_pem: str,
    device_label: str,
) -> tuple[dict[str, Any], str, str]:
    ca = load_or_generate_ca(ca_dir())
    client_cert_pem, fingerprint = sign_csr(ca, csr_pem, device_label)

    state = LinkState.load_or_create()
    paired_at = _utc_now_iso()
    _authorized().add(
        fingerprint=fingerprint,
        device_label=device_label,
        instance_id=state.instance_id,
        role=consumed.role,
        paired_at=paired_at,
    )
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
    return response, fingerprint, paired_at


def _emit_pair_complete(
    device_label: str,
    fingerprint: str,
    paired_at: str,
) -> None:
    emit(
        "link",
        "pair_complete",
        device_label=device_label,
        fingerprint=fingerprint,
        fingerprint_short=fingerprint.replace("sha256:", "")[:16],
        paired_at=paired_at,
    )


@link_bp.route("/pair", methods=["POST"])
def pair() -> Any:
    """Mobile pair endpoint — accepts CSR + nonce, signs + mints attestation.

    Query: `?token=<nonce>` (the nonce minted by /pair-start).
    Body  (JSON):
        {
          "csr":          "<PEM>",      // required
          "device_label": "<string>",   // optional (falls back to nonce label)
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

    consumed = _nonces().consume(nonce_value)
    if consumed is None:
        return error_response(
            OPERATION_NO_LONGER_AVAILABLE,
            detail="nonce expired or used",
        )

    effective_label = device_label or (consumed.device_label or _default_device_label())

    try:
        response, fingerprint, paired_at = _complete_pairing(
            consumed,
            csr_pem,
            effective_label,
        )
    except ValueError as exc:
        logger.info("pair: bad csr: %s", exc)
        return error_response(PAIRING_KEY_INVALID, detail=f"bad csr: {exc}")
    _emit_pair_complete(effective_label, fingerprint, paired_at)
    return jsonify(response)


@link_bp.route("/by-code", methods=["POST"])
def by_code() -> Any:
    """Mobile pair endpoint — accepts CSR + manual code."""
    body = request.get_json(silent=True) or {}
    code = body.get("code")
    csr_pem = body.get("csr")
    device_label = str(body.get("device_label") or "").strip()

    if not isinstance(code, str) or not isinstance(csr_pem, str):
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="missing fields (code + csr required)",
        )

    canonical_code = normalize_manual_code(code)
    if not MANUAL_CODE_RE.fullmatch(canonical_code):
        return error_response(PAIRING_REQUEST_INVALID, detail="bad code")

    consumed = _nonces().consume_by_code(canonical_code)
    if consumed is None:
        return error_response(
            OPERATION_NO_LONGER_AVAILABLE,
            detail="nonce expired or used",
        )

    effective_label = device_label or consumed.device_label or _default_device_label()
    try:
        response, fingerprint, paired_at = _complete_pairing(
            consumed,
            csr_pem,
            effective_label,
        )
    except ValueError as exc:
        logger.info("by-code: bad csr: %s", exc)
        return error_response(PAIRING_KEY_INVALID, detail=f"bad csr: {exc}")
    _emit_pair_complete(effective_label, fingerprint, paired_at)
    return jsonify(response)


@link_bp.route("/unpair", methods=["POST"])
def unpair() -> Any:
    """Revoke a paired device by label or fingerprint.

    Body (JSON): {"fingerprint": "sha256:..."} or {"device_label": "..."}
    """
    body = request.get_json(silent=True) or {}
    fingerprint = body.get("fingerprint")
    device_label = body.get("device_label")
    if not isinstance(fingerprint, str):
        if not isinstance(device_label, str):
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="fingerprint or device_label required",
            )
        entry = _authorized().find_by_label(device_label)
        if entry is None:
            return error_response(
                PAIRED_DEVICE_NOT_FOUND,
                detail="no paired device with that label",
            )
        fingerprint = entry.fingerprint

    removed = _authorized().remove(fingerprint)
    if not removed:
        return error_response(PAIRED_DEVICE_NOT_FOUND, detail="fingerprint not paired")
    return jsonify({"unpaired": fingerprint})


def _entry_to_json(entry: ClientEntry) -> dict[str, Any]:
    short_fp = entry.fingerprint.replace("sha256:", "")[:16]
    return {
        "fingerprint": entry.fingerprint,
        "fingerprint_short": short_fp,
        "device_label": entry.device_label,
        "paired_at": entry.paired_at,
        "last_seen_at": entry.last_seen_at,
    }


# ---------------------------------------------------------------------------
# helpers for the workspace template
# ---------------------------------------------------------------------------


@link_bp.app_context_processor
def _inject_link_helpers() -> dict[str, Any]:
    """Make `url_for` to link endpoints easy from templates."""
    return {"link_copy": link_copy}
