#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Container-only fault injector for the public-v1 crossover reference.

if [ "$#" -eq 3 ] \
    && [ "$1" = "--user" ] \
    && [ "$2" = "stop" ] \
    && [ "$3" = "solstone.service" ]; then
    echo "injected reference fault: refusing legacy service stop" >&2
    exit 1
fi

exec /usr/bin/systemctl.reference-real "$@"
