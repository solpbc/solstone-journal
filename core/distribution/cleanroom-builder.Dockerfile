# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Builder subject: rustc 1.97.1 + zig 0.16.0 on the Python-absent bookworm
# pin. The digest is the inventory [[cleanroom.subject]] pin; do not float.

FROM debian:bookworm-slim@sha256:362e64223cc0da95422b3b13c045186fc0a81250e765d31c025fbddf257f6143

LABEL solstone.cleanroom.subject="debian-bookworm-no-python" \
      solstone.cleanroom.builder="rust-zig-no-python" \
      solstone.cleanroom.rustc="1.97.1" \
      solstone.cleanroom.zig="0.16.0" \
      solstone.cleanroom.python="absent" \
      solstone.cleanroom.network="none"
