# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import select
import shutil
import sys
from typing import TextIO

from solstone.observe.peer_lookup import PeerInfo
from solstone.observe.pl_http import PlHttpSession


def maybe_prompt_unpair(
    peer: PeerInfo,
    session: PlHttpSession,
    *,
    stdin: TextIO = sys.stdin,
    stdout: TextIO = sys.stdout,
) -> None:
    prompt = f'Unpair "{peer.label}" now? (y/N) '
    stdout.write(prompt)
    stdout.flush()

    if stdin.isatty():
        answer = stdin.readline().strip()
    else:
        readable, _, _ = select.select([stdin], [], [], 5.0)
        if not readable:
            stdout.write(f'Keeping peer "{peer.label}" (non-interactive default).\n')
            stdout.flush()
            return
        answer = stdin.readline().strip()

    if answer.lower() == "y":
        _do_unpair(peer, session, stdout=stdout)
        return
    stdout.write(f'Keeping peer "{peer.label}".\n')
    stdout.flush()


def _do_unpair(
    peer: PeerInfo,
    session: PlHttpSession,
    *,
    stdout: TextIO = sys.stdout,
) -> None:
    response = session.post(
        "/app/link/unpair",
        json={"fingerprint": peer.cert_fingerprint},
    )
    if response.status_code == 200:
        shutil.rmtree(peer.dir)
        stdout.write(f'Unpaired "{peer.label}".\n')
        stdout.flush()
        return
    stdout.write(
        f'Failed to unpair "{peer.label}": HTTP {response.status_code}: '
        f"{response.content[:200].decode('utf-8', errors='replace')}\n"
    )
    stdout.flush()
