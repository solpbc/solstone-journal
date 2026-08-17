# Payload

The product data the shipped binary reads at runtime: talents, prompt
templates, the journal contract bundle, the skill tree, AMD attestation roots.
`core/distribution/payload.txt` is the allow-list, and
`core/distribution/inventory.toml` names this directory as `payload_src_root`.

The subtree below is called `solstone` because that is the shipped layout's own
name — every path here lands under `share/solstone/` in an installed tree, and
one set of relative paths describes both. So this directory is the checkout's
stand-in for the installed `share/` prefix, not a copy of the Python package of
the same name.

Everything here ships. Nothing here is source for anything else. Two files are
generated rather than hand-written — `solstone/talent/journal/contract/bundle.json`
by `solstone-core contract build`, and the two `references/commands.md` by
`scripts/build_skill_references.py` — and both are checked in CI.
