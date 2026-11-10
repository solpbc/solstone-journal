#!/bin/sh
# Print the changed-area commands `make ci` does not run, filled in for the
# workspace packages this tree changes against origin/main. Print only: this
# script never fails the gate, and says so when it cannot compute the list.
set -u

say() { printf '%s\n' "$*"; }

if ! base=$(git merge-base HEAD origin/main 2>/dev/null); then
	say "Changed-area commands: not computed (no origin/main ref to compare against)."
	exit 0
fi

changed=$( { git diff --no-renames --name-only "$base" -- core/crates; git ls-files --others --exclude-standard -- core/crates; } 2>/dev/null \
	| sed -n 's#^core/crates/\([^/]*\)/.*#\1#p' | sort -u)

packages=""
features=""
for package in $changed; do
	manifest="core/crates/$package/Cargo.toml"
	[ -f "$manifest" ] || continue
	packages="$packages $package"
	for feature in full-tests test-hooks; do
		if sed -n '/^\[features\]/,/^\[/p' "$manifest" | grep -q "^$feature *="; then
			features="${features:+$features,}$package/$feature"
		fi
	done
done

# Talent payload and fixture files sit outside core/crates, but the served-byte
# contracts pin them: convey-shell's convey_route_contracts and thinking_corpus,
# and solstone-core's core_journal_contracts. A change to them owes those suites.
data=$( { git diff --no-renames --name-only "$base" -- core/payload core/fixtures tests/fixtures; git ls-files --others --exclude-standard -- core/payload core/fixtures tests/fixtures; } 2>/dev/null)
consumers=""
if [ -n "$data" ]; then
	for package in solstone-core-convey-shell solstone-core; do
		case " $packages " in
		*" $package "*) ;;
		*) consumers="$consumers $package" ;;
		esac
	done
fi

# A moved platform gate (#[cfg(windows)], cfg!(unix), target_os, ...) in a shared
# crate changes what other targets compile, and make ci builds only this host's.
# On 2026-11-06 one such change broke the iOS build and reached the release.
gates=$(git diff --no-renames -U0 "$base" -- core/crates 2>/dev/null \
	| grep -E '^[+-][^+-].*cfg!?\(.*(target_os|target_family|target_vendor|windows|unix)' || true)
case "$gates" in
*windows*) windows_gate=1 ;;
*) windows_gate="" ;;
esac

if [ -z "$packages$consumers" ]; then
	say "Changed-area commands: no workspace package changed against origin/main."
	exit 0
fi

say "Changed-area commands for the packages this tree changes against origin/main (make ci ran none of these):"
[ -n "$consumers" ] && say "  (payload or fixture files changed, so the served-byte contract packages are included:$consumers)"
[ -n "$packages" ] && say "  cargo clippy --manifest-path core/Cargo.toml$(for p in $packages; do printf ' -p %s' "$p"; done)${features:+ --features $features} --all-targets --locked -- -D warnings"
case " $packages " in
*" solstone-core-repository-contracts "*) suites="$packages$consumers" ;;
*) suites="$packages$consumers solstone-core-repository-contracts" ;;
esac
if [ -n "$gates" ]; then
	say "  (this tree moves a platform cfg gate in a shared crate, so other targets owe their own compile:)"
	say "  make check-rust-ios   # on a Mac (pro5e), before landing"
	[ -n "$windows_gate" ] && say "  make win-host-ci      # on sol-winbuild, before landing"
fi
if [ -n "${HOPPER_LID:-}" ]; then
	say "  (ci-full is not supported inside this automated build; run the scoped ci-full after landing)"
else
	say "  make ci-full PACKAGES=\"$(printf '%s' "$suites" | sed 's/^ //; s/ /,/g')\""
	say "  (keep solstone-core-repository-contracts in that list: its full-tests scan every crate's sources, so a change elsewhere can red it)"
fi
exit 0
