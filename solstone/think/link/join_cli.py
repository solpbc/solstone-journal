# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Caller-side `sol link join` implementation.

Manual short-code form posts to `/app/link/by-code`; v3 pair-link URL form
decodes the embedded nonce and posts to `/app/link/pair?token=<nonce>`.

Observer credentials are written under
`$XDG_CONFIG_HOME/solstone-observer/spl/<label>/` when XDG_CONFIG_HOME is set,
otherwise `~/.config/solstone-observer/spl/<label>/`.

Peer credentials are written under `<journal_root>/peers/<instance_id>/`,
where `instance_id` is the receiver instance_id returned by the pair response,
not the local `--label`. Label-to-instance_id resolution for
`journal transfer send --to <label>` is a follow-on lode that will walk
`peer.json` files.

Both layouts contain `private.pem`, `cert.pem`, `chain.pem`,
`home_attestation.jwt`, and `peer.json`. `peer.json` fields are deterministic:
`label`, `paired_at`, `instance_id`, `home_label`, `fingerprint`,
`local_endpoints`, and `role`; role is `observer` or `peer` for documented join
storage layouts.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import ipaddress
import json
import os
import re
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from solstone.apps.link.copy import PAIR_LINK_HOST, PAIR_LINK_PATH
from solstone.apps.link.crockford32 import decode as crockford_decode
from solstone.apps.link.manual_code import normalize as normalize_manual_code
from solstone.think.link.paths import LinkState
from solstone.think.utils import get_journal

VALID_ROLES = {"phone", "observer", "peer"}
MANUAL_CODE_RE = re.compile(r"^[0-9A-HJKMNP-TV-Z]{8}$")
LABEL_RE = re.compile(r"^[A-Za-z0-9_.-]+$")
BUNDLE_FILES = {
    "private.pem",
    "cert.pem",
    "chain.pem",
    "home_attestation.jwt",
    "peer.json",
}
_INSTANCE_ID_RE = re.compile(r"^[A-Za-z0-9-]{1,256}$")


@dataclass(frozen=True)
class PairRequest:
    url: str
    body_base: dict[str, str]


@dataclass(frozen=True)
class PairResponse:
    client_cert: str
    ca_chain: list[str]
    instance_id: str
    home_label: str
    home_attestation: str
    local_endpoints: list[Any]


def add_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--home", help="Receiver base URL")
    parser.add_argument("--code", required=True, help="Manual code or pair-link URL")
    parser.add_argument("--as", required=True, dest="as_role", help="Role to join as")
    parser.add_argument("--label", required=True, help="Local credentials label")


def main(args: argparse.Namespace) -> int:
    if args.as_role not in VALID_ROLES:
        return _fail("invalid role; expected one of: phone, observer, peer", code=2)

    label = str(args.label)
    label_error = _label_error(label)
    if label_error is not None:
        return _fail(label_error, code=2)

    try:
        pair_request = _parse_pair_request(str(args.code).strip(), args.home)
    except ValueError as exc:
        return _fail(str(exc), code=1)

    if args.as_role == "peer":
        private_key_pem, csr_pem = _build_csr(label)
        body = {
            **pair_request.body_base,
            "csr": csr_pem,
            "device_label": label,
        }
        body["sender_instance_id"] = LinkState.load_or_create().instance_id
        try:
            response = _post_pair(pair_request.url, body)
        except ValueError as exc:
            return _fail(str(exc), code=1)
        instance_id_error = _validate_instance_id(response.instance_id)
        if instance_id_error is not None:
            return _fail(instance_id_error, code=1)
        bundle_dir = _peer_dir(response.instance_id)
        existing_error = _existing_dir_error(bundle_dir)
        if existing_error is not None:
            return _fail(existing_error, code=1)
    else:
        bundle_dir = _bundle_dir(label)
        existing_error = _existing_dir_error(bundle_dir)
        if existing_error is not None:
            return _fail(existing_error, code=1)

        private_key_pem, csr_pem = _build_csr(label)
        body = {
            **pair_request.body_base,
            "csr": csr_pem,
            "device_label": label,
        }
        try:
            response = _post_pair(pair_request.url, body)
        except ValueError as exc:
            return _fail(str(exc), code=1)

    chain_pem = _join_chain(response.ca_chain)
    try:
        ca_fp = _ca_fingerprint(chain_pem)
    except ValueError as exc:
        return _fail(str(exc), code=1)

    peer = {
        "label": label,
        "paired_at": dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "instance_id": response.instance_id,
        "home_label": response.home_label,
        "fingerprint": ca_fp,
        "local_endpoints": response.local_endpoints,
        "role": args.as_role,
    }
    files = {
        "private.pem": private_key_pem,
        "cert.pem": response.client_cert.encode("utf-8"),
        "chain.pem": chain_pem.encode("utf-8"),
        "home_attestation.jwt": response.home_attestation.encode("utf-8"),
        "peer.json": (json.dumps(peer, indent=2) + "\n").encode("utf-8"),
    }
    created_dir = not bundle_dir.exists()
    try:
        _write_bundle(bundle_dir, files, created_dir=created_dir)
    except OSError as exc:
        return _fail(str(exc), code=1)

    print(f"Linked {label} as {args.as_role}.")
    print(f"Credentials: {bundle_dir}")
    return 0


def _parse_pair_request(code: str, home: str | None) -> PairRequest:
    if code.startswith(f"https://{PAIR_LINK_HOST}{PAIR_LINK_PATH}#"):
        return _parse_pair_link(code, home)
    canonical_code = normalize_manual_code(code)
    if not MANUAL_CODE_RE.fullmatch(canonical_code):
        raise ValueError("Invalid pair code")
    if not home:
        raise ValueError("--home is required for manual pair codes")
    base_url = home.rstrip("/")
    return PairRequest(
        url=f"{base_url}/app/link/by-code",
        body_base={"code": canonical_code},
    )


def _parse_pair_link(pair_link: str, home: str | None) -> PairRequest:
    parsed = urllib.parse.urlparse(pair_link)
    fragment = parsed.fragment
    try:
        blob = crockford_decode(fragment)
    except ValueError as exc:
        raise ValueError("Invalid pair link") from exc
    if len(blob) != 40 or blob[0] != 0x04 or blob[1] != 0x01:
        raise ValueError("Invalid pair link")
    ipv4 = str(ipaddress.IPv4Address(blob[2:6]))
    port = int.from_bytes(blob[6:8], "big")
    nonce_hex = blob[8:24].hex()
    base_url = home.rstrip("/") if home else f"https://{ipv4}:{port}"
    return PairRequest(
        url=f"{base_url}/app/link/pair?token={nonce_hex}",
        body_base={},
    )


def _label_error(label: str) -> str | None:
    if not label:
        return "--label must not be empty"
    if len(label) > 80:
        return "--label must be 80 characters or fewer"
    if "/" in label or "\\" in label:
        return "--label must not contain path separators"
    if ".." in label:
        return "--label must not contain '..'"
    if label.startswith("."):
        return "--label must not start with '.'"
    if not LABEL_RE.fullmatch(label):
        return "--label may contain only letters, numbers, '-', '_', and '.'"
    return None


def _validate_instance_id(value: str) -> str | None:
    if not _INSTANCE_ID_RE.fullmatch(value):
        return f"bad instance_id from receiver: {value!r}"
    return None


def _bundle_dir(label: str) -> Path:
    config_home = os.environ.get("XDG_CONFIG_HOME")
    base = Path(config_home) if config_home else Path.home() / ".config"
    return base / "solstone-observer" / "spl" / label


def _peer_dir(instance_id: str) -> Path:
    return Path(get_journal()) / "peers" / instance_id


def _existing_dir_error(bundle_dir: Path) -> str | None:
    if not bundle_dir.exists():
        return None
    for entry in bundle_dir.iterdir():
        name = entry.name
        if (
            not name.startswith(".")
            or name in BUNDLE_FILES
            or name.lstrip(".") in BUNDLE_FILES
        ):
            return (
                f"Credentials directory already exists with content: {bundle_dir}. "
                f"Remove with 'rm -rf {bundle_dir}' and rerun if re-pairing."
            )
    return None


def _build_csr(label: str) -> tuple[bytes, str]:
    private_key = ec.generate_private_key(ec.SECP256R1())
    csr = (
        x509.CertificateSigningRequestBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, label[:64])]))
        .sign(private_key, hashes.SHA256())
    )
    private_key_pem = private_key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    csr_pem = csr.public_bytes(serialization.Encoding.PEM).decode("ascii")
    return private_key_pem, csr_pem


def _post_pair(url: str, body: dict[str, str]) -> PairResponse:
    data = json.dumps(body).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=data,
        headers={"content-type": "application/json"},
        method="POST",
    )
    # This is the trust-on-first-use join ceremony. The returned CA chain is
    # persisted for future verification by the caller-side runtime.
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    try:
        with urllib.request.urlopen(request, timeout=30, context=context) as response:
            status = int(getattr(response, "status", response.getcode()))
            raw_body = response.read()
    except urllib.error.HTTPError as exc:
        excerpt = exc.read().decode("utf-8", errors="replace")[:500]
        raise ValueError(
            f"Pair request failed with HTTP {exc.code}: {excerpt}"
        ) from exc
    except urllib.error.URLError as exc:
        raise ValueError(f"Pair request failed: {exc.reason}") from exc
    if status != 200:
        excerpt = raw_body.decode("utf-8", errors="replace")[:500]
        raise ValueError(f"Pair request failed with HTTP {status}: {excerpt}")
    try:
        payload = json.loads(raw_body.decode("utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise ValueError("Pair response was not valid JSON") from exc
    return _parse_pair_response(payload)


def _parse_pair_response(payload: Any) -> PairResponse:
    if not isinstance(payload, dict):
        raise ValueError("Pair response was not a JSON object")
    client_cert = _required_str(payload, "client_cert")
    ca_chain = payload.get("ca_chain")
    if not isinstance(ca_chain, list) or not ca_chain:
        raise ValueError("Pair response missing ca_chain")
    if not all(isinstance(item, str) and item for item in ca_chain):
        raise ValueError("Pair response field ca_chain is invalid")
    instance_id = _required_str(payload, "instance_id")
    home_attestation = _required_str(payload, "home_attestation")
    home_label = payload.get("home_label")
    local_endpoints = payload.get("local_endpoints")
    return PairResponse(
        client_cert=client_cert,
        ca_chain=ca_chain,
        instance_id=instance_id,
        home_label=home_label if isinstance(home_label, str) else "",
        home_attestation=home_attestation,
        local_endpoints=local_endpoints if isinstance(local_endpoints, list) else [],
    )


def _required_str(payload: dict[str, Any], field: str) -> str:
    value = payload.get(field)
    if not isinstance(value, str) or not value:
        raise ValueError(f"Pair response missing {field}")
    return value


def _join_chain(ca_chain: list[str]) -> str:
    return "".join(cert if cert.endswith("\n") else f"{cert}\n" for cert in ca_chain)


def _ca_fingerprint(chain_pem: str) -> str:
    cert_pem = _first_cert_pem(chain_pem)
    cert = x509.load_pem_x509_certificate(cert_pem.encode("ascii"))
    der = cert.public_bytes(serialization.Encoding.DER)
    return f"sha256:{hashlib.sha256(der).hexdigest()}"


def _first_cert_pem(chain_pem: str) -> str:
    marker = "-----BEGIN CERTIFICATE-----"
    start = chain_pem.find(marker)
    if start < 0:
        raise ValueError("CA chain contained no certificate")
    end_marker = "-----END CERTIFICATE-----"
    end = chain_pem.find(end_marker, start)
    if end < 0:
        raise ValueError("CA chain contained an incomplete certificate")
    end += len(end_marker)
    return chain_pem[start:end] + "\n"


def _write_bundle(
    bundle_dir: Path,
    files: dict[str, bytes],
    *,
    created_dir: bool,
) -> None:
    written: list[Path] = []
    try:
        bundle_dir.mkdir(parents=True, exist_ok=True)
        bundle_dir.chmod(0o700)
        for name, content in files.items():
            path = bundle_dir / name
            _write_bytes(path, content)
            written.append(path)
    except OSError:
        for path in written:
            try:
                path.unlink()
            except OSError:
                pass
        if created_dir:
            try:
                bundle_dir.rmdir()
            except OSError:
                pass
        raise


def _write_bytes(path: Path, content: bytes) -> None:
    try:
        with open(path, "wb") as f:
            f.write(content)
            f.flush()
            os.fsync(f.fileno())
        path.chmod(0o600)
    except OSError as exc:
        raise OSError(f"failed to write {path}: {exc}") from exc


def _fail(message: str, *, code: int) -> int:
    print(message, file=sys.stderr)
    return code
