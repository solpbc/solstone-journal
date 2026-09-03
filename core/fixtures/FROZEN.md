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

## Externally pinned, versioned oracles

A second class of fixture under `core/fixtures/` is **externally pinned,
versioned oracles**: produced outside this repository by an independent
implementation (not the Rust code under test), each carrying an embedded receipt
naming the exact upstream sources and tool versions used to construct it.

- Immutable once committed. Do not edit one in place, even alongside a behavior
  change that consumes it.
- A change to what it pins (model, tokenizer, chat template, or construction
  method) requires a new, independently produced and pinned fixture at a new
  versioned filename, committed before its consumer change lands. Prior versions
  remain intact; never overwrite an existing fixture.
- `qwen35_admission_oracle.json` is the first fixture in this class.

The rules above remain unchanged for producerless frozen regression pins. This
second class is governed by its immutable rules instead.
