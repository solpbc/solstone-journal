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
      nonces.json      pair-ceremony nonces (5-min TTL, single-use)
      state.json       instance_id + home_label (generated on first run)

`journal/link/` is a narrow exception to the "memories live in
day/stream/segment/" rule — this is config, not memory, scoped to this
one service.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from solstone.think.journal_io import write_json
from solstone.think.utils import get_journal

# Production spl-relay endpoint. Single source of truth; self-hosters
# override via SOL_LINK_RELAY_URL env var.
DEFAULT_RELAY_URL = "https://link.solstone.app"


def link_root() -> Path:
    """`journal/link/` — auto-created."""
    root = Path(get_journal()) / "link"
    root.mkdir(parents=True, exist_ok=True)
    return root


def ca_dir() -> Path:
    d = link_root() / "ca"
    d.mkdir(parents=True, exist_ok=True)
    return d


def staging_dir() -> Path:
    """Pure path for staged candidate CA material."""
    return Path(get_journal()) / "link" / "ca-staging"


def authorized_clients_path() -> Path:
    return link_root() / "authorized_clients.json"


def service_token_path() -> Path:
    return Path(get_journal()) / "link" / "tokens" / "account.json"


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
        # Intended fail-closed-on-unreadable-config: use the benign default relay.
        pass
    return DEFAULT_RELAY_URL


@dataclass
class LinkState:
    """Service identity — the values spl-relay binds a service_token to.

    Persisted to `journal/link/state.json`; generated on first run.
    """

    instance_id: str
    home_label: str
    locked_at: int | None = None

    @property
    def jid(self) -> str:
        return self.instance_id

    @classmethod
    def load_or_create(cls, *, default_label: str = "solstone") -> LinkState:
        from solstone.think.link import establish

        return establish.create_link_state(default_label=default_label)

    @classmethod
    def load(cls, *, default_label: str = "solstone") -> LinkState | None:
        """Pure read of `state.json`; None if unprovisioned/unreadable. No write."""
        path = Path(get_journal()) / "link" / "state.json"
        if not path.exists():
            return None
        try:
            raw = json.loads(path.read_text("utf-8"))
            iid = raw.get("instance_id")
            label = raw.get("home_label") or default_label
            value = raw.get("locked_at")
            locked_at = value if isinstance(value, int) else None
            if isinstance(iid, str) and iid:
                return cls(instance_id=iid, home_label=label, locked_at=locked_at)
        except (json.JSONDecodeError, OSError):
            return None
        return None

    def save(self) -> None:
        payload: dict[str, object] = {
            "instance_id": self.instance_id,
            "home_label": self.home_label,
        }
        if self.locked_at is not None:
            payload["locked_at"] = self.locked_at
        write_json(state_path(), payload, indent=2)


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
    write_json(path, {"service_token": token}, indent=2, mode=0o600)
