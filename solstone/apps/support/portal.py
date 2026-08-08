# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Welcome-mat client for support.solstone.app.

Implements the full DPoP + self-signed access token auth flow per the
welcome-mat spec.

All cryptographic operations use the ``cryptography`` library (already a
solstone dependency).  The keypair, access token, and cached TOS are
persisted in the journal's app storage directory.
"""

from __future__ import annotations

import base64
import hashlib
import json
import logging
import os
import time
import uuid
from pathlib import Path
from typing import Any

import httpx
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa

from solstone.apps.support import operations
from solstone.apps.support.copy import FEEDBACK_SUBJECT

logger = logging.getLogger(__name__)

# Owner-product brand tier. The portal moved from support.solpbc.org to
# support.solstone.app (2026-05-17). The legacy host still serves the agent
# API and the portal accepts both audiences, so installs with an older
# ``support.portal_url`` in journal config keep working until they pick this
# default up on upgrade.
DEFAULT_PORTAL_URL = "https://support.solstone.app"
SUPPORT_PORTAL_URL_ENV = "SOLSTONE_SUPPORT_URL"

# ---------------------------------------------------------------------------
# Base64url helpers
# ---------------------------------------------------------------------------


def _b64url_encode(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def _b64url_decode(s: str) -> bytes:
    s += "=" * (4 - len(s) % 4)
    return base64.urlsafe_b64decode(s)


def _rewind_files(files: dict[str, Any]) -> None:
    """Rewind file-like multipart values before a bounded TOS retry."""
    for value in files.values():
        file_value = value[1] if isinstance(value, tuple) and len(value) > 1 else value
        seek = getattr(file_value, "seek", None)
        if seek is not None:
            seek(0)


def _response_json_object(resp: httpx.Response) -> dict[str, Any] | None:
    """Return a response JSON object when it is safe to classify a status."""
    try:
        data = resp.json()
    except ValueError:
        return None
    return data if isinstance(data, dict) else None


def _remote_operation_id(data: dict[str, Any]) -> str | None:
    """Select the remote id for later acknowledgement bookkeeping."""
    for field in ("ticket_id", "message_id", "attachment_id", "id"):
        value = data.get(field)
        if value is not None:
            return str(value)
    return None


# ---------------------------------------------------------------------------
# JWT helpers
# ---------------------------------------------------------------------------


def _jwt_encode(header: dict, payload: dict, private_key: rsa.RSAPrivateKey) -> str:
    """Create a signed JWT (RS256)."""
    h = _b64url_encode(json.dumps(header, separators=(",", ":")).encode())
    p = _b64url_encode(json.dumps(payload, separators=(",", ":")).encode())
    signing_input = f"{h}.{p}".encode()
    sig = private_key.sign(signing_input, padding.PKCS1v15(), hashes.SHA256())
    return f"{h}.{p}.{_b64url_encode(sig)}"


def _sha256_b64url(data: str | bytes) -> str:
    if isinstance(data, str):
        data = data.encode("utf-8")
    return _b64url_encode(hashlib.sha256(data).digest())


# ---------------------------------------------------------------------------
# JWK / Thumbprint
# ---------------------------------------------------------------------------


def _public_key_jwk(key: rsa.RSAPublicKey) -> dict:
    """Export RSA public key as a JWK dict."""
    numbers = key.public_numbers()
    e_bytes = numbers.e.to_bytes((numbers.e.bit_length() + 7) // 8, "big")
    n_bytes = numbers.n.to_bytes((numbers.n.bit_length() + 7) // 8, "big")
    return {
        "kty": "RSA",
        "e": _b64url_encode(e_bytes),
        "n": _b64url_encode(n_bytes),
    }


def _jwk_thumbprint(jwk: dict) -> str:
    """RFC 7638 JWK thumbprint (SHA-256, base64url)."""
    # Canonical JSON: alphabetical keys
    canonical = json.dumps(
        {"e": jwk["e"], "kty": "RSA", "n": jwk["n"]},
        separators=(",", ":"),
        sort_keys=True,
    )
    return _sha256_b64url(canonical)


# ---------------------------------------------------------------------------
# Portal client
# ---------------------------------------------------------------------------


class PortalClient:
    """Welcome-mat client for the support portal.

    Parameters
    ----------
    portal_url:
        Base URL of the portal (no trailing slash). Defaults to the configured
        support portal URL, with ``SOLSTONE_SUPPORT_URL`` taking precedence.
    storage_dir:
        Directory for keypair, token cache, and TOS cache.
    handle:
        Agent handle for registration.  Defaults to the machine hostname.
    anonymous:
        If True, generate a random handle and don't persist the keypair.
    """

    def __init__(
        self,
        portal_url: str | None = None,
        storage_dir: Path | None = None,
        handle: str | None = None,
        anonymous: bool = False,
    ) -> None:
        if portal_url is None:
            portal_url = _get_portal_url_from_settings()
        self.portal_url = portal_url.rstrip("/")
        self.anonymous = anonymous
        self._handle = handle
        if self.anonymous and not self._handle:
            self._handle = f"anon-{os.urandom(4).hex()}"

        if storage_dir is None:
            from solstone.apps.utils import get_app_storage_path

            storage_dir = get_app_storage_path("support", "portal", ensure_exists=True)

        self.storage_dir = Path(storage_dir)
        self.storage_dir.mkdir(parents=True, exist_ok=True)

        self._private_key: rsa.RSAPrivateKey | None = None
        self._access_token: str | None = None
        self._tos_text: str | None = None
        self._jwk: dict | None = None
        self._thumbprint: str | None = None

        self._load_state()

    # -- Persistence ---------------------------------------------------------

    @property
    def _keypair_path(self) -> Path:
        return self.storage_dir / "keypair.pem"

    @property
    def _token_path(self) -> Path:
        return self.storage_dir / "token.json"

    @property
    def _tos_cache_path(self) -> Path:
        return self.storage_dir / "tos.txt"

    @property
    def handle(self) -> str:
        if self._handle:
            return self._handle
        import socket

        hostname = socket.gethostname().lower().replace("_", "-")[:48]
        # Ensure valid handle format
        handle = "".join(c for c in hostname if c.isalnum() or c in ".-")
        handle = handle.strip(".-") or "solstone"
        self._handle = f"solstone-{handle}"
        return self._handle

    def _load_state(self) -> None:
        """Load persisted keypair and token."""
        if self.anonymous:
            return

        if self._keypair_path.is_file():
            pem = self._keypair_path.read_bytes()
            self._private_key = serialization.load_pem_private_key(pem, password=None)
            pub = self._private_key.public_key()
            self._jwk = _public_key_jwk(pub)
            self._thumbprint = _jwk_thumbprint(self._jwk)

        if self._token_path.is_file():
            try:
                data = json.loads(self._token_path.read_text())
                self._access_token = data.get("access_token")
                self._handle = data.get("handle", self._handle)
            except (json.JSONDecodeError, OSError):
                pass

        if self._tos_cache_path.is_file():
            try:
                self._tos_text = self._tos_cache_path.read_text()
            except OSError:
                pass

    def _save_keypair(self) -> None:
        if self.anonymous or self._private_key is None:
            return
        pem = self._private_key.private_bytes(
            encoding=serialization.Encoding.PEM,
            format=serialization.PrivateFormat.PKCS8,
            encryption_algorithm=serialization.NoEncryption(),
        )
        self._keypair_path.write_bytes(pem)
        self._keypair_path.chmod(0o600)

    def _save_token(self) -> None:
        if self.anonymous:
            return
        data = {"access_token": self._access_token, "handle": self._handle}
        self._token_path.write_text(json.dumps(data))

    def _save_tos(self, tos_text: str) -> None:
        self._tos_text = tos_text
        if not self.anonymous:
            self._tos_cache_path.write_text(tos_text)

    # -- Key management ------------------------------------------------------

    def _ensure_keypair(self) -> None:
        """Generate RSA-4096 keypair if we don't have one."""
        if self._private_key is not None:
            return

        logger.info("Generating RSA-4096 keypair for support portal registration")
        self._private_key = rsa.generate_private_key(
            public_exponent=65537,
            key_size=4096,
        )
        pub = self._private_key.public_key()
        self._jwk = _public_key_jwk(pub)
        self._thumbprint = _jwk_thumbprint(self._jwk)
        self._save_keypair()

    # -- DPoP proof creation -------------------------------------------------

    def _create_dpop_proof(
        self,
        method: str,
        url: str,
        access_token: str | None = None,
    ) -> str:
        """Create a DPoP proof JWT per RFC 9449."""
        assert self._private_key is not None
        assert self._jwk is not None

        header = {
            "typ": "dpop+jwt",
            "alg": "RS256",
            "jwk": self._jwk,
        }
        payload: dict[str, Any] = {
            "jti": str(uuid.uuid4()),
            "htm": method,
            "htu": url.split("?")[0],  # strip query/fragment
            "iat": int(time.time()),
        }
        if access_token is not None:
            payload["ath"] = _sha256_b64url(access_token)

        return _jwt_encode(header, payload, self._private_key)

    # -- Access token creation -----------------------------------------------

    def _create_access_token(self, tos_text: str) -> str:
        """Create a self-signed wm+jwt access token."""
        assert self._private_key is not None
        assert self._thumbprint is not None

        header = {"typ": "wm+jwt", "alg": "RS256"}
        payload = {
            "jti": str(uuid.uuid4()),
            "tos_hash": _sha256_b64url(tos_text),
            "aud": self.portal_url,
            "cnf": {"jkt": self._thumbprint},
            "iat": int(time.time()),
        }
        return _jwt_encode(header, payload, self._private_key)

    # -- TOS signing ---------------------------------------------------------

    def _sign_tos(self, tos_text: str) -> str:
        """Sign TOS text with RS256 and return base64url signature."""
        assert self._private_key is not None
        sig = self._private_key.sign(
            tos_text.encode("utf-8"),
            padding.PKCS1v15(),
            hashes.SHA256(),
        )
        return _b64url_encode(sig)

    # -- HTTP helpers --------------------------------------------------------

    def _http(self) -> httpx.Client:
        return httpx.Client(timeout=30.0)

    @staticmethod
    def _raise_for_status(resp: httpx.Response) -> None:
        """Like resp.raise_for_status() but includes the response body."""
        try:
            resp.raise_for_status()
        except httpx.HTTPStatusError as exc:
            detail = resp.text[:500] if resp.text else ""
            raise httpx.HTTPStatusError(
                f"{exc.request.method} {exc.request.url} — {resp.status_code}: {detail}",
                request=exc.request,
                response=exc.response,
            ) from None

    def _authed_headers(self, method: str, url: str) -> dict[str, str]:
        """Return Authorization + DPoP headers for an authenticated request."""
        assert self._access_token is not None
        return {
            "Authorization": f"DPoP {self._access_token}",
            "DPoP": self._create_dpop_proof(method, url, self._access_token),
        }

    def _principal(self) -> str:
        """Return the stable ledger principal for this portal client."""
        if self.anonymous:
            return "anonymous"
        self._ensure_keypair()
        assert self._thumbprint is not None
        return f"jkt:{self._thumbprint}"

    def _authed_request(
        self,
        method: str,
        path: str,
        *,
        json_body: dict | None = None,
        params: dict | None = None,
        files: dict[str, Any] | None = None,
        idempotency_key: str | None = None,
        retry_on_tos: bool = True,
    ) -> httpx.Response:
        """Make an authenticated request, handling TOS re-consent."""
        url = f"{self.portal_url}{path}"
        headers = self._authed_headers(method, url)
        if idempotency_key is not None:
            headers["Idempotency-Key"] = idempotency_key

        with self._http() as client:
            if files is not None:
                _rewind_files(files)
                resp = client.request(
                    method, url, headers=headers, files=files, params=params
                )
            else:
                resp = client.request(
                    method, url, headers=headers, json=json_body, params=params
                )

        if resp.status_code == 401 and retry_on_tos:
            try:
                body = resp.json()
            except Exception:
                body = {}
            if body.get("error") == "tos_changed":
                logger.info("TOS changed — re-registering")
                self.register()
                return self._authed_request(
                    method,
                    path,
                    json_body=json_body,
                    params=params,
                    files=files,
                    idempotency_key=idempotency_key,
                    retry_on_tos=False,
                )

        return resp

    # -- Public API ----------------------------------------------------------

    @property
    def is_registered(self) -> bool:
        return self._access_token is not None and self._private_key is not None

    @property
    def cached_tos(self) -> str | None:
        """Return locally cached TOS text, or None if not cached."""
        return self._tos_text

    def fetch_tos(self) -> str:
        """Fetch the current TOS from the portal."""
        url = f"{self.portal_url}/tos"
        with self._http() as client:
            resp = client.get(url, headers={"Accept": "text/plain"})
            self._raise_for_status(resp)
        tos_text = resp.text
        self._save_tos(tos_text)
        return tos_text

    def register(self, _retry_count: int = 0) -> dict[str, Any]:
        """Run the full welcome-mat registration flow.

        1. Ensure keypair exists
        2. Fetch TOS
        3. Sign TOS
        4. Create access token
        5. POST /api/signup
        """
        self._ensure_keypair()

        tos_text = self.fetch_tos()
        tos_signature = self._sign_tos(tos_text)
        access_token = self._create_access_token(tos_text)

        url = f"{self.portal_url}/api/signup"
        dpop_proof = self._create_dpop_proof("POST", url)

        body = {
            "tos_signature": tos_signature,
            "access_token": access_token,
            "handle": self.handle,
        }

        with self._http() as client:
            resp = client.post(
                url,
                headers={"DPoP": dpop_proof, "Content-Type": "application/json"},
                json=body,
            )

        if resp.status_code == 409:
            if _retry_count >= 3:
                raise RuntimeError(
                    "could not register — all handle variants were taken after 3 attempts"
                )
            import random
            import re
            import string

            base = re.sub(r"-[a-z0-9]{4}$", "", self._handle)
            suffix = "".join(
                random.choices(string.ascii_lowercase + string.digits, k=4)
            )
            self._handle = f"{base}-{suffix}"
            return self.register(_retry_count=_retry_count + 1)

        self._raise_for_status(resp)
        data = resp.json()

        self._access_token = data["access_token"]
        self._handle = data.get("handle", self._handle)
        self._save_token()

        logger.info("Registered with support portal as %s", self._handle)
        return data

    def ensure_registered(self) -> None:
        """Register if not already registered."""
        if not self.is_registered:
            self.register()

    def _dispatch_mutation(
        self,
        method: str,
        path: str,
        *,
        action_id: str,
        verb: str,
        fields: dict[str, Any],
        index: int = 0,
        json_body: dict | None = None,
        files: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Dispatch one durable mutation through the local outcome matrix.

        Transport failures, 5xx responses, malformed successful JSON, and a
        remote in-progress response remain locally in progress for an exact-key
        retry.  Well-formed success completes the record; the portal's terminal
        conflict, state, retirement, erasure, and repeated-TOS responses mark it
        failed before raising their typed local error.
        """
        record = operations.begin_operation(
            action_id,
            verb,
            fields,
            principal=self._principal(),
            index=index,
            storage_dir=self.storage_dir,
        )
        if record.state == "pending":
            record = operations.mark_in_progress(record, storage_dir=self.storage_dir)
        try:
            resp = self._authed_request(
                method,
                path,
                json_body=json_body,
                files=files,
                idempotency_key=record.operation_key,
            )
        except httpx.TransportError:
            operations.release_retryable_lease(record, storage_dir=self.storage_dir)
            raise

        if 200 <= resp.status_code < 300:
            try:
                data = resp.json()
            except ValueError:
                operations.release_retryable_lease(record, storage_dir=self.storage_dir)
                raise
            if not isinstance(data, dict):
                operations.release_retryable_lease(record, storage_dir=self.storage_dir)
                raise ValueError("support portal mutation response must be an object")
            remote_operation_id = _remote_operation_id(data)
            operations.mark_completed(
                record,
                remote_operation_id=remote_operation_id,
                storage_dir=self.storage_dir,
            )
            return data

        if resp.status_code >= 500:
            operations.release_retryable_lease(record, storage_dir=self.storage_dir)
            self._raise_for_status(resp)

        body = _response_json_object(resp)
        error = body.get("error") if body is not None else None
        if resp.status_code == 409 and error == "operation_in_progress":
            raise operations.OperationInProgressError()
        if resp.status_code == 409 and error == "idempotency_conflict":
            operations.mark_failed(
                record,
                reason="idempotency_conflict",
                storage_dir=self.storage_dir,
            )
            raise operations.IdempotencyConflictError()
        if resp.status_code == 409 and error == "invalid_state":
            operations.mark_failed(
                record,
                reason="invalid_state",
                storage_dir=self.storage_dir,
            )
            raise operations.OperationInvalidStateError()
        if resp.status_code == 410 and error == "operation_retired":
            operations.mark_failed(
                record,
                reason="operation_retired",
                storage_dir=self.storage_dir,
            )
            raise operations.OperationRetiredError()
        if error == "operation_erased":
            operations.mark_failed(
                record,
                reason="operation_erased",
                storage_dir=self.storage_dir,
            )
            raise operations.OperationErasedError()
        if resp.status_code == 401 and error == "tos_changed":
            operations.mark_failed(
                record,
                reason="tos_changed",
                storage_dir=self.storage_dir,
            )
            raise operations.OperationTosChangedError()

        operations.release_retryable_lease(record, storage_dir=self.storage_dir)
        self._raise_for_status(resp)
        raise AssertionError("non-success support mutation response was not raised")

    # -- Tickets -------------------------------------------------------------

    def create_ticket(
        self,
        *,
        product: str = "solstone",
        subject: str,
        description: str,
        severity: str = "medium",
        category: str | None = None,
        user_email: str | None = None,
        user_context: dict | str | None = None,
        action_id: str,
    ) -> dict[str, Any]:
        """Create a support ticket."""
        self.ensure_registered()
        body: dict[str, Any] = {
            "product": product,
            "subject": subject,
            "description": description,
            "severity": severity,
        }
        if category is not None:
            body["category"] = category
        if user_email is not None:
            body["user_email"] = user_email
        if user_context is not None:
            body["user_context"] = user_context
        fields = dict(body)
        fields["anonymous"] = self.anonymous
        return self._dispatch_mutation(
            "POST",
            "/api/tickets",
            action_id=action_id,
            verb="create",
            fields=fields,
            json_body=body,
        )

    def list_tickets(
        self,
        *,
        status: str | None = None,
        product: str | None = None,
        severity: str | None = None,
    ) -> list[dict[str, Any]]:
        """List tickets (own tickets for user accounts)."""
        self.ensure_registered()
        params: dict[str, str] = {}
        if status:
            params["status"] = status
        if product:
            params["product"] = product
        if severity:
            params["severity"] = severity

        resp = self._authed_request("GET", "/api/tickets", params=params)
        self._raise_for_status(resp)
        return resp.json()

    def get_ticket(self, ticket_id: int) -> dict[str, Any]:
        """Get a single ticket with message thread."""
        self.ensure_registered()
        resp = self._authed_request("GET", f"/api/tickets/{ticket_id}")
        self._raise_for_status(resp)
        return resp.json()

    def reply_to_ticket(
        self, ticket_id: int, content: str, *, action_id: str
    ) -> dict[str, Any]:
        """Add a message to a ticket."""
        self.ensure_registered()
        body = {"content": content}
        return self._dispatch_mutation(
            "POST",
            f"/api/tickets/{ticket_id}/messages",
            action_id=action_id,
            verb="reply",
            fields={"ticket_id": ticket_id, "content": content},
            json_body=body,
        )

    def submit_feedback(
        self,
        *,
        body: str,
        product: str = "solstone",
        user_email: str | None = None,
        user_context: dict | str | None = None,
        action_id: str,
    ) -> dict[str, Any]:
        """Submit lower-friction feedback through its own ledger verb."""
        self.ensure_registered()
        payload: dict[str, Any] = {
            "product": product,
            "subject": FEEDBACK_SUBJECT,
            "description": body,
            "severity": "low",
            "category": "feedback",
        }
        if user_email is not None:
            payload["user_email"] = user_email
        if user_context is not None:
            payload["user_context"] = user_context
        fields: dict[str, Any] = {
            "product": product,
            "body": body,
            "anonymous": self.anonymous,
        }
        if user_email is not None:
            fields["user_email"] = user_email
        if user_context is not None:
            fields["user_context"] = user_context
        return self._dispatch_mutation(
            "POST",
            "/api/tickets",
            action_id=action_id,
            verb="feedback",
            fields=fields,
            json_body=payload,
        )

    # -- Attachments ---------------------------------------------------------

    MAX_ATTACHMENT_SIZE = 10 * 1024 * 1024  # 10 MB
    MAX_ATTACHMENTS_PER_MESSAGE = 5

    ALLOWED_CONTENT_TYPES = {
        ".png": "image/png",
        ".jpg": "image/jpeg",
        ".jpeg": "image/jpeg",
        ".gif": "image/gif",
        ".webp": "image/webp",
        ".svg": "image/svg+xml",
        ".pdf": "application/pdf",
        ".txt": "text/plain",
        ".csv": "text/csv",
        ".html": "text/html",
        ".md": "text/markdown",
        ".xml": "text/xml",
        ".json": "application/json",
    }

    def attach_file(
        self,
        ticket_id: int,
        file_path: Path,
        *,
        action_id: str,
        index: int = 0,
        filename: str | None = None,
        content_type: str | None = None,
    ) -> dict[str, Any]:
        """Upload a file attachment to a ticket.

        Parameters
        ----------
        ticket_id:
            The ticket to attach the file to.
        file_path:
            Path to the local file.
        filename:
            Override filename sent to the portal (defaults to file_path.name).
        content_type:
            Override MIME type (auto-detected from extension if omitted).

        Raises
        ------
        ValueError
            If the file is too large or has an unsupported type.
        FileNotFoundError
            If the file does not exist.
        """
        self.ensure_registered()
        file_path = Path(file_path)

        if not file_path.is_file():
            raise FileNotFoundError(f"File not found: {file_path}")

        size = file_path.stat().st_size
        if size > self.MAX_ATTACHMENT_SIZE:
            raise ValueError(
                f"File too large: {size / 1024 / 1024:.1f} MB "
                f"(max {self.MAX_ATTACHMENT_SIZE / 1024 / 1024:.0f} MB)"
            )

        if content_type is None:
            suffix = file_path.suffix.lower()
            content_type = self.ALLOWED_CONTENT_TYPES.get(suffix)
            if content_type is None:
                raise ValueError(
                    f"Unsupported file type: {suffix}. "
                    f"Allowed: {', '.join(sorted(self.ALLOWED_CONTENT_TYPES))}"
                )

        fname = filename or file_path.name
        with file_path.open("rb") as file_handle:
            digest = hashlib.sha256()
            snapshot_size = 0
            while chunk := file_handle.read(1024 * 1024):
                snapshot_size += len(chunk)
                if snapshot_size > self.MAX_ATTACHMENT_SIZE:
                    raise ValueError(
                        f"File too large: {snapshot_size / 1024 / 1024:.1f} MB "
                        f"(max {self.MAX_ATTACHMENT_SIZE / 1024 / 1024:.0f} MB)"
                    )
                digest.update(chunk)
            file_handle.seek(0)
            return self._dispatch_mutation(
                "POST",
                f"/api/tickets/{ticket_id}/attachments",
                action_id=action_id,
                verb="attach",
                fields={
                    "ticket_id": ticket_id,
                    "filename": fname,
                    "content_type": content_type,
                    "byte_size": snapshot_size,
                    "content_sha256": digest.hexdigest(),
                },
                index=index,
                files={"file": (fname, file_handle, content_type)},
            )

    # -- Knowledge Base ------------------------------------------------------

    def search_articles(self, query: str | None = None) -> list[dict[str, Any]]:
        """Search published KB articles."""
        self.ensure_registered()
        params: dict[str, str] = {}
        if query:
            params["q"] = query

        resp = self._authed_request("GET", "/api/articles", params=params)
        self._raise_for_status(resp)
        return resp.json()

    def get_article(self, slug: str) -> dict[str, Any]:
        """Read a single KB article."""
        self.ensure_registered()
        resp = self._authed_request("GET", f"/api/articles/{slug}")
        self._raise_for_status(resp)
        return resp.json()

    # -- Announcements -------------------------------------------------------

    def list_announcements(self) -> list[dict[str, Any]]:
        """List active announcements."""
        self.ensure_registered()
        resp = self._authed_request("GET", "/api/announcements")
        self._raise_for_status(resp)
        return resp.json()

    # -- Health --------------------------------------------------------------

    def health(self) -> dict[str, Any]:
        """Check portal health (no auth needed)."""
        with self._http() as client:
            resp = client.get(f"{self.portal_url}/api/health")
            self._raise_for_status(resp)
        return resp.json()


# -- Module-level convenience ------------------------------------------------


def get_client(
    portal_url: str | None = None,
    anonymous: bool = False,
) -> PortalClient:
    """Get a portal client using environment/journal settings for configuration.

    Reads ``SOLSTONE_SUPPORT_URL`` first, then ``support.portal_url`` from
    journal config, if *portal_url* is None.
    """
    if portal_url is None:
        portal_url = _get_portal_url_from_settings()
    return PortalClient(portal_url=portal_url, anonymous=anonymous)


def _get_portal_url_from_settings() -> str:
    """Resolve portal URL from env, then journal config, then the default host."""
    env_url = os.environ.get(SUPPORT_PORTAL_URL_ENV)
    if env_url:
        return env_url.rstrip("/")

    try:
        from solstone.think.utils import get_journal

        config_path = Path(get_journal()) / "config" / "config.json"
        if config_path.is_file():
            config = json.loads(config_path.read_text())
            support = config.get("support", {})
            url = support.get("portal_url")
            if url:
                return url.rstrip("/")
    except Exception:
        pass
    return DEFAULT_PORTAL_URL


def is_enabled() -> bool:
    """Check if the support agent is enabled in settings."""
    try:
        from solstone.think.utils import get_journal

        config_path = Path(get_journal()) / "config" / "config.json"
        if config_path.is_file():
            config = json.loads(config_path.read_text())
            support = config.get("support", {})
            return support.get("enabled", True)
    except Exception:
        pass
    return True
