# Frozen fixtures

These oracles have no producer. Their generators were the Python reference,
which is gone. They are **frozen regression pins**, not regenerable goldens.

- Do not delete a fixture because you cannot find its generator.
- If the Rust behaviour it pins legitimately changes, edit the fixture **in
  the same commit as the behaviour change**, and say why in the commit body.

**But not everything in a frozen corpus is a behaviour pin.** A served frontend
asset's body (`text/html`, `text/javascript`, `text/css` at status 200) is not
product behaviour: pinning its bytes asserts that a file we deliberately edit
has not been edited, so it reddens on every legitimate UI change and says
nothing about what changed. Those pins were removed from the conveyance corpora
on 2026-09-03.

- A record with **no `body_sha256`** is deliberately not body-asserted. Do not
  "repair" it by adding one back.
- For those routes the corpus still asserts status, content type, content
  disposition and `Location` — that is the contract worth freezing.
- `normalized-json` API response bodies remain fully asserted. This carve-out
  is about served asset files only.
- To assert frontend **content**, target the thing you care about. The stronger
  form already in the tree is `superseded_presentation_asset` in
  `solstone-core-convey-shell/tests/thinking_corpus.rs`, which compares the
  served body against the asset on disk rather than a recorded digest, so it
  cannot go stale.
- If you find yourself hand-patching a hash so CI goes green, stop and ask
  whether that pin was ever an assertion about the product.

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

### Hybrid product-wire/provider observations

`qwen35_b10068_wire_oracle_v1.json` is immutable but has deliberately mixed
provenance: its exact request bodies were captured from the pinned production
Generate and Converse builders, while its rendered prompts, token vectors, and
three agreeing token-count observations were produced by the pinned external
b10068 llama.cpp process. The embedded product commit and source hashes are a
historical receipt for those captured bodies, not assertions about future
repository revisions.

A change to either side requires a new versioned fixture. Never edit this v1
file in place or describe its product-derived request bodies as independently
implemented.

The rules above remain unchanged for producerless frozen regression pins. This
second class is governed by its immutable rules instead.
