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

if [ -z "$packages" ]; then
	say "Changed-area commands: no workspace package changed against origin/main."
	exit 0
fi

say "Changed-area commands for the packages this tree changes against origin/main (make ci ran none of these):"
say "  cargo clippy --manifest-path core/Cargo.toml$(for p in $packages; do printf ' -p %s' "$p"; done)${features:+ --features $features} --all-targets --locked -- -D warnings"
case " $packages " in
*" solstone-core-repository-contracts "*) suites="$packages" ;;
*) suites="$packages solstone-core-repository-contracts" ;;
esac
if [ -n "${HOPPER_LID:-}" ]; then
	say "  (ci-full is not supported inside this automated build; run the scoped ci-full after landing)"
else
	say "  make ci-full PACKAGES=\"$(printf '%s' "$suites" | sed 's/^ //; s/ /,/g')\""
fi
exit 0
