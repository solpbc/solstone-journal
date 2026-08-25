#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

set -eu

GIT=${GIT:-git}

unmerged=$("$GIT" ls-files --unmerged)
if [ -n "$unmerged" ]; then
  echo "ERROR: refusing Windows tree sync: index contains unmerged entries:" >&2
  printf '%s\n' "$unmerged" | awk -F '	' '{ print $2 }' | sort -u | sed 's/^/  /' >&2
  echo "Resolve or abort the merge before retrying." >&2
  exit 1
fi

untracked=$("$GIT" ls-files --others --exclude-standard)
if [ -n "$untracked" ]; then
  echo "ERROR: refusing Windows tree sync: untracked non-ignored files would be omitted from the git bundle:" >&2
  printf '%s\n' "$untracked" | sed 's/^/  /' >&2
  echo "Run 'git add <path>' to include them, or ignore/remove them before retrying." >&2
  exit 1
fi
