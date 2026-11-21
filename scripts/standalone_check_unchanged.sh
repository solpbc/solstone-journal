#!/bin/sh
# Exit 0 when a standalone check can be skipped in `make ci`: this tree changes
# something against origin/main, and none of it is under the given paths.
#
# A standalone crate has its own lockfile and no path dependency on the
# workspace, so a change elsewhere cannot move its result. A tree with no
# change at all is main itself (a release cut, a cloud run): there the check
# runs, so main stays checked on every host. Anything this script cannot work
# out also runs the check.
set -u

[ "$#" -gt 0 ] || exit 1
base=$(git merge-base HEAD origin/main 2>/dev/null) || exit 1

changed() {
	[ -n "$(git diff --no-renames --name-only "$base" -- "$@" 2>/dev/null; git ls-files --others --exclude-standard -- "$@" 2>/dev/null)" ]
}

# The toolchain pin and the Makefile recipe decide how every check runs.
changed "$@" rust-toolchain.toml Makefile && exit 1
changed . || exit 1
exit 0
