# Frozen fixtures

These oracles have no producer. Their generators were the Python reference,
which is gone. They are **frozen regression pins**, not regenerable goldens.

- Do not delete a fixture because you cannot find its generator.
- If the Rust behaviour it pins legitimately changes, edit the fixture **in
  the same commit as the behaviour change**, and say why in the commit body.

This applies to every file under `core/fixtures/` that a test treats as an
oracle — the top-level `*.json` corpora, the nested `native-sol/`,
`body-source/`, `pdf_corpus/`, and journal trees included.

`mark_derivation_contract.json` is the same class: its generator
(`scripts/build_mark_derivation_contract.py`) is gone. Do not regenerate it.