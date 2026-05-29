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
import time
from dataclasses import asdict, dataclass
from importlib import import_module
from pathlib import Path
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
from solstone.apps.link.relay_link import (
    TOTP_STEP_SECONDS,
    compute_current_totp,
    encode_relay_pair_link,
)
from solstone.apps.observer.utils import mint_pl_observer_record, revoke_observer_record
from solstone.apps.utils import log_app_action
from solstone.convey import emit
from solstone.convey.network_access import (
    NetworkAccessPasswordRequired,
    NetworkAccessPasswordTooShort,
    set_network_access,
)
from solstone.convey.reasons import (
    CONVEY_OPERATION_FAILED,
    INVALID_CONFIG_VALUE,
    INVALID_OPERATION_FOR_STATE,
    INVALID_REQUEST_VALUE,
    MISSING_REQUIRED_FIELD,
    NETWORK_SECURITY_REQUIRES_PASSWORD,
    OPERATION_NO_LONGER_AVAILABLE,
    PAIRED_DEVICE_NOT_FOUND,
    PAIRING_KEY_INVALID,
    PAIRING_REQUEST_INVALID,
)
from solstone.convey.utils import error_response
from solstone.think.link.auth import AuthorizedClients, ClientEntry
from solstone.think.link.ca import (
    generate_nonce,
    generate_relay_nonce,
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
    DEFAULT_RELAY_URL,
    LinkState,
    authorized_clients_path,
    ca_dir,
    load_service_token,
    load_totp_secret,
    nonces_path,
    relay_url,
)
from solstone.think.link.window import read_posture
from solstone.think.utils import get_config, get_journal, now_ms

logger = logging.getLogger(__name__)
MANUAL_CODE_RE = re.compile(rf"^[0-9A-HJKMNP-TV-Z]{{{MANUAL_CODE_LEN}}}$")
_SENDER_INSTANCE_ID_RE = re.compile(r"^[A-Za-z0-9-]{1,256}$")
VALID_ROLES = {"phone", "observer", "peer"}
# The watcher emits only lan/ula today; vpn stays empty until a scope is wired.
VPN_SCOPES = {"vpn"}
journal_sources = import_module("solstone.apps.import.journal_sources")
create_state_directory = journal_sources.create_state_directory
load_journal_source_by_fingerprint = journal_sources.load_journal_source_by_fingerprint
save_journal_source = journal_sources.save_journal_source
journal_source_state_prefix = journal_sources.journal_source_state_prefix
mint_pl_journal_source_record = journal_sources.mint_pl_journal_source_record

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


def _convey_password_is_set() -> bool:
    password_hash = get_config().get("convey", {}).get("password_hash", "")
    return bool(str(password_hash or "").strip())


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
    """Build the v3 pair-link URL.

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


def _derive_relay_state(token_present: bool) -> str:
    """Return pre-mechanism relay attachment state.

    connecting/parked are valid contract values but are not produced until
    parking is wired.
    """
    return "offline" if token_present else "not-enrolled"


def _derive_reachability(
    lan_accessible: bool,
    posture: str,
    relay_state: str,
) -> str:
    if not lan_accessible:
        return "lan-unreachable"
    if posture == "direct":
        return "online"
    # posture == "spl": map relay_state. "reconnecting" is reserved.
    return {
        "connecting": "finishing-setup",
        "parked": "online",
        "offline": "offline",
        "not-enrolled": "finishing-setup",
    }[relay_state]


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
    token = load_service_token()
    token_present = token is not None
    ca_fp = _ca_fingerprint() if ca_dir().exists() else None
    lan_accessible = _is_lan_accessible()
    posture = read_posture()
    relay_state = _derive_relay_state(token_present)
    reachability = _derive_reachability(lan_accessible, posture, relay_state)
    home_address = _resolve_host_port() if lan_accessible else None
    vpn_candidates = [
        {"label": ep.scope, "address": f"{ep.ip}:{ep.port}"}
        for ep in _current_local_endpoints()
        if ep.scope in VPN_SCOPES
    ]
    return jsonify(
        {
            "instance_id": state.instance_id,
            "home_label": state.home_label,
            "enrolled": token_present,
            "relay_url": relay_url(),
            "ca_fingerprint": ca_fp,
            "has_password": _convey_password_is_set(),
            "lan_accessible": lan_accessible,
            "posture": posture,
            "reachability": reachability,
            "relay_state": relay_state,
            "home_address": home_address,
            "vpn": {"active": None, "candidates": vpn_candidates},
        }
    )


@link_bp.route("/network-access", methods=["POST"])
def network_access() -> Any:
    payload = request.get_json(silent=True) or {}
    if not isinstance(payload, dict):
        payload = {}
    raw_password = payload.get("password")
    password = raw_password if isinstance(raw_password, str) and raw_password else None
    try:
        result = set_network_access(enable=True, password=password)
    except NetworkAccessPasswordRequired:
        return error_response(NETWORK_SECURITY_REQUIRES_PASSWORD)
    except NetworkAccessPasswordTooShort:
        return error_response(
            INVALID_CONFIG_VALUE,
            detail="Password must be at least 8 characters",
        )
    except Exception:
        logger.exception("link network access enable failed")
        return error_response(CONVEY_OPERATION_FAILED)
    return jsonify(result)


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

    nonce_ttl: int | None = None
    if read_posture() == "spl":
        secret = load_totp_secret()
        if secret is None:
            return error_response(
                INVALID_OPERATION_FOR_STATE,
                detail="spl posture requires a relay TOTP secret; none is configured",
            )

        ca = load_or_generate_ca(ca_dir())
        ca_fp = ca.fingerprint_sha256()
        now = int(time.time())
        totp = compute_current_totp(secret, now)
        nonce = generate_relay_nonce()
        origin = relay_url()
        relay_origin = None if origin == DEFAULT_RELAY_URL else origin
        instance_id = LinkState.load_or_create().instance_id
        pair_link = encode_relay_pair_link(
            instance_id,
            totp,
            nonce,
            ca.spki_fingerprint_sha256(),
            relay_origin=relay_origin,
        )
        expires_in = TOTP_STEP_SECONDS
        nonce_ttl = TOTP_STEP_SECONDS
    else:
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
        pair_link = _build_pair_link(hostname, port, nonce, ca_fp)
        expires_in = 300

    manual_code_hyphenated = generate_manual_code()
    add_kwargs: dict[str, Any] = {}
    if nonce_ttl is not None:
        add_kwargs["ttl"] = nonce_ttl
    _nonces().add(
        nonce,
        device_label,
        role=role,
        manual_code=normalize_manual_code(manual_code_hyphenated),
        **add_kwargs,
    )
    response = PairStartResponse(
        nonce=nonce,
        pair_link=pair_link,
        manual_code=manual_code_hyphenated,
        expires_in=expires_in,
        device_label=device_label,
        lan_url=lan_url,
        ca_fingerprint=ca_fp,
    )
    return _jsonify_preserving_order(asdict(response))


def _complete_pairing(
    consumed: Nonce,
    csr_pem: str,
    device_label: str,
    *,
    sender_instance_id: str | None = None,
) -> tuple[dict[str, Any], str, str]:
    ca = load_or_generate_ca(ca_dir())
    client_cert_pem, fingerprint = sign_csr(ca, csr_pem, device_label)

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

    observer_record_path = None
    journal_source_record_path = None
    try:
        if consumed.role == "peer":
            journal_source_record_path = mint_pl_journal_source_record(
                fingerprint=fingerprint,
                device_label=device_label,
                paired_at=paired_at,
                peer_instance_id=sender_instance_id,
            )
            create_state_directory(Path(get_journal()), journal_source_record_path.stem)
        if consumed.role == "observer":
            observer_record_path = mint_pl_observer_record(
                fingerprint=fingerprint,
                device_label=device_label,
                paired_at=paired_at,
            )
        _authorized().add(
            fingerprint=fingerprint,
            device_label=device_label,
            instance_id=state.instance_id,
            role=consumed.role,
            paired_at=paired_at,
        )
    except Exception:
        if observer_record_path is not None:
            try:
                observer_record_path.unlink()
            except FileNotFoundError:
                pass
        if journal_source_record_path is not None:
            try:
                journal_source_record_path.unlink()
            except FileNotFoundError:
                pass
        raise

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

    effective_label = device_label or (consumed.device_label or _default_device_label())

    try:
        response, fingerprint, paired_at = _complete_pairing(
            consumed,
            csr_pem,
            effective_label,
            sender_instance_id=sender_instance_id,
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
            sender_instance_id=sender_instance_id,
        )
    except ValueError as exc:
        logger.info("by-code: bad csr: %s", exc)
        return error_response(PAIRING_KEY_INVALID, detail=f"bad csr: {exc}")
    _emit_pair_complete(effective_label, fingerprint, paired_at)
    return jsonify(response)


@link_bp.route("/rename", methods=["POST"])
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
    if not updated:
        return error_response(PAIRED_DEVICE_NOT_FOUND, detail="fingerprint not paired")
    return jsonify({"fingerprint": fingerprint, "label": label.strip()})


@link_bp.route("/unpair", methods=["POST"])
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
    if fingerprint is not None:
        entry = authorized.get(fingerprint)
    elif device_label is not None:
        entry = authorized.find_by_label(device_label)
        if entry is not None:
            fingerprint = entry.fingerprint
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

    fp_hex = fingerprint.removeprefix("sha256:")
    short_fp = fp_hex[:16]
    role = entry.role

    if role == "phone":
        removed = authorized.remove(fingerprint)
        if not removed:
            logger.warning(
                "unpair: phone entry %s already absent from authorized_clients",
                short_fp,
            )
    elif role == "observer":
        try:
            revoke_observer_record(short_fp)
        except ValueError as exc:
            msg = str(exc)
            if "already revoked" in msg:
                logger.warning("unpair: observer %s already revoked: %s", short_fp, msg)
            else:
                logger.warning(
                    "unpair: observer record missing for %s: %s", short_fp, msg
                )
            authorized.remove(fingerprint)
        except RuntimeError as exc:
            logger.error(
                "unpair: failed to save observer record for %s: %s",
                short_fp,
                exc,
            )
            authorized.remove(fingerprint)
    elif role == "peer":
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
        logger.warning(
            "unpair: unexpected role %r for entry %s; treating as phone",
            role,
            short_fp,
        )
        authorized.remove(fingerprint)
    return jsonify({"unpaired": fingerprint})


def _entry_to_json(entry: ClientEntry) -> dict[str, Any]:
    short_fp = entry.fingerprint.replace("sha256:", "")[:16]
    return {
        "fingerprint": entry.fingerprint,
        "fingerprint_short": short_fp,
        "device_label": entry.device_label,
        "paired_at": entry.paired_at,
        "last_seen_at": entry.last_seen_at,
        "role": entry.role,
    }


# ---------------------------------------------------------------------------
# helpers for the workspace template
# ---------------------------------------------------------------------------


@link_bp.app_context_processor
def _inject_link_helpers() -> dict[str, Any]:
    """Make `url_for` to link endpoints easy from templates."""
    return {"link_copy": link_copy}
