# journal mark — vendored asset sets (LOCKED)

These three files are the **canonical, frozen** asset sets for the journal mark — the deterministic
visual derived from a journal's identity. The mark must render identically on every surface and
platform, so these sets are **locked**: do not reorder, regenerate, add, or remove entries. Index
position is load-bearing (the derivation selects by `value % len`).

- `glyphs.json` — 60 distinct Lucide icons (name → inline SVG inner markup). Order = enumeration order.
- `colors.json` — 16 `[name, hex]` color entries. Order load-bearing.
- `words.json`  — the EFF "long" wordlist (7776 words). Order load-bearing.

## attribution / licenses

- **Lucide icons** — ISC License. Source: https://github.com/lucide-icons/lucide (lucide-static).
  The 60-icon subset was curated for visual distinctness; full set is also vendored at
  `core/crates/solstone-core-convey-shell/assets/static/icons/` with its license.
- **EFF Long Wordlist** — Creative Commons Attribution 3.0 US (CC BY 3.0 US). Source:
  https://www.eff.org/dice ("EFF's New Wordlists for Random Passphrases", 2016).
- **Color list** — sol pbc; no third-party rights.
