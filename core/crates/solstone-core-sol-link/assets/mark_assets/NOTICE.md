# journal mark — vendored asset sets (LOCKED)

These three files are the **canonical, frozen** asset sets for the journal mark — the deterministic
visual derived from a journal's identity. The mark must render identically on every surface and
platform, so these sets are **locked**: do not reorder, regenerate, add, or remove entries. Index
position is load-bearing (the derivation selects by `value % len`).

- `glyphs.json` — 60 distinct Lucide icons (name → inline SVG inner markup). Order = enumeration order.
- `colors.json` — 16 `[name, hex]` color entries. Order load-bearing.
- `words.json`  — the EFF "long" wordlist (7776 words). Order load-bearing.

## attribution / licenses

- **Lucide icons** — ISC License, and MIT for the icons Lucide inherits from Feather. Five of the
  60 are among those: `feather`, `key`, `lock`, `music`, `tv`. Both texts are below. Source:
  https://github.com/lucide-icons/lucide (lucide-static).
  The 60-icon subset was curated for visual distinctness; full set is also vendored at
  `core/crates/solstone-core-convey-shell/assets/static/icons/` with its license.
- **EFF Long Wordlist** — Creative Commons Attribution 3.0 US (CC BY 3.0 US). Source:
  https://www.eff.org/dice ("EFF's New Wordlists for Random Passphrases", 2016).
- **Color list** — sol pbc; no third-party rights.

## Lucide license texts

```
ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

```
The MIT License (MIT) (for the icons listed above)

Copyright (c) 2013-present Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
