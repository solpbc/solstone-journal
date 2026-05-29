# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""journal/link/ path resolution + service state I/O.

All link-service state lives under `journal/link/`:

    journal/link/
      ca/
        cert.pem       world-readable local CA cert
        private.pem    mode 0600 — filesystem-perms-only protection
      authorized_clients.json   paired-device ledger (mtime-reloaded)
      tokens/
        account.json   cached service_token from /enroll/home
      totp.json      mode 0600 relay pairing TOTP secret
      nonces.json      pair-ceremony nonces (5-min TTL, single-use)
      state.json       instance_id + home_label (generated on first run)

`journal/link/` is a narrow exception to the "memories live in
day/stream/segment/" rule — this is config, not memory, scoped to this
one service (see cpo/strategy/journal-memory-structure.md).
"""

from __future__ import annotations

import base64
import json
import os
import secrets
import uuid
from dataclasses import dataclass
from pathlib import Path

from solstone.think.utils import get_journal

# Production spl-relay endpoint. Single source of truth — self-hosters
# override via SOL_LINK_RELAY_URL env var. When CTO wires
# spl.solpbc.org as DNS front, update this constant.
DEFAULT_RELAY_URL = "https://spl-relay-staging.jer-3f2.workers.dev"


def link_root() -> Path:
    """`journal/link/` — auto-created."""
    root = Path(get_journal()) / "link"
    root.mkdir(parents=True, exist_ok=True)
    return root


def ca_dir() -> Path:
    d = link_root() / "ca"
    d.mkdir(parents=True, exist_ok=True)
    return d


def authorized_clients_path() -> Path:
    return link_root() / "authorized_clients.json"


def tokens_dir() -> Path:
    d = link_root() / "tokens"
    d.mkdir(parents=True, exist_ok=True)
    return d


def service_token_path() -> Path:
    return tokens_dir() / "account.json"


def totp_secret_path() -> Path:
    return link_root() / "totp.json"


def nonces_path() -> Path:
    return link_root() / "nonces.json"


def state_path() -> Path:
    return link_root() / "state.json"


def relay_url() -> str:
    """Resolve the spl-relay endpoint.

    Precedence: SOL_LINK_RELAY_URL env var > journal config `link.relay_url` >
    DEFAULT_RELAY_URL constant. Self-hosters override one-field; production
    users get the default.
    """
    env = os.environ.get("SOL_LINK_RELAY_URL", "").strip()
    if env:
        return env.rstrip("/")
    try:
        from solstone.think.utils import get_config

        cfg = get_config()
        link_cfg = cfg.get("link") if isinstance(cfg, dict) else None
        if isinstance(link_cfg, dict):
            url = link_cfg.get("relay_url")
            if isinstance(url, str) and url.strip():
                return url.strip().rstrip("/")
    except Exception:
        pass
    return DEFAULT_RELAY_URL


@dataclass
class LinkState:
    """Service identity — the values spl-relay binds a service_token to.

    Persisted to `journal/link/state.json`; generated on first run.
    """

    instance_id: str
    home_label: str

    @classmethod
    def load_or_create(cls, *, default_label: str = "solstone") -> LinkState:
        path = state_path()
        if path.exists():
            try:
                raw = json.loads(path.read_text("utf-8"))
                iid = raw.get("instance_id")
                label = raw.get("home_label") or default_label
                if isinstance(iid, str) and iid:
                    return cls(instance_id=iid, home_label=label)
            except (json.JSONDecodeError, OSError):
                pass
        state = cls(instance_id=str(uuid.uuid4()), home_label=default_label)
        state.save()
        return state

    def save(self) -> None:
        path = state_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(
                {"instance_id": self.instance_id, "home_label": self.home_label},
                f,
                indent=2,
            )
            f.write("\n")
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)


def load_service_token() -> str | None:
    """Read the cached /enroll/home service token, or None."""
    path = service_token_path()
    if not path.exists():
        return None
    try:
        raw = json.loads(path.read_text("utf-8"))
        # back-compat: pre-rename caches stored the token under "account_token"
        token = raw.get("service_token") or raw.get("account_token")
        return token if isinstance(token, str) and token else None
    except (json.JSONDecodeError, OSError):
        return None


def save_service_token(token: str) -> None:
    """Persist the service token atomically with mode 0600."""
    path = service_token_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".json.tmp")
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump({"service_token": token}, f, indent=2)
        f.write("\n")
        f.flush()
        os.fsync(f.fileno())
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def generate_totp_secret() -> str:
    return base64.b32encode(secrets.token_bytes(20)).decode("ascii").rstrip("=")


def load_totp_secret() -> str | None:
    """Read the relay pairing TOTP secret, or None."""
    path = totp_secret_path()
    if not path.exists():
        return None
    try:
        raw = json.loads(path.read_text("utf-8"))
        secret = raw.get("totp_secret")
        return secret if isinstance(secret, str) and secret else None
    except (json.JSONDecodeError, OSError):
        return None


def save_totp_secret(secret: str) -> None:
    """Persist the relay pairing TOTP secret atomically with mode 0600."""
    path = totp_secret_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".json.tmp")
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump({"totp_secret": secret}, f, indent=2)
        f.write("\n")
        f.flush()
        os.fsync(f.fileno())
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)
