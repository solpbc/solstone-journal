# W5: native image import source

## Scope and deliberate differences

Implement `solstone-core-import-sources/src/image.rs` as the native owner of
one imported still-image segment.  Its durable authority is deliberately
narrow: install the owner's private original and write its description
transcript under `chronicle/<day>/import.image/<segment>/`.

Three differences from the Python reference are intentional:

- The native order is install original, describe, then write transcript.  A
  model outage therefore preserves the original and produces an explanatory
  transcript instead of leaving no segment.
- The transcript has a marked, structurally testable model-derived region.
  Python's current image transcript has no such marker; this follows the
  document-import precedent so model-derived text is distinguishable from
  deterministic metadata.
- Python derives the manifest day list from output-path positions and this
  segment layout produces an empty list. Native supplies the explicit one-item
  `[day]` list instead, so manifest metadata records the segment it wrote.

`import_image` is a write verb; it replaces the reserved seam rather than
adding a `process` alias.  `preview` and `detect` remain read-only.  There is
no `dry_run` argument or branch: preview is this module's dry surface, and the
audited importer call shape does not pass `dry_run` to image processing.

## Module surface

- `detect(path: &Path) -> bool` returns true only for a regular file whose
  extension, folded case-insensitively, is one of PNG, JPG, JPEG, WEBP, GIF,
  or TIFF.  This is the advertised grammar set; TIFF remains detectable even
  though the currently enabled native decoder cannot decode it.
- `preview(path: &Path) -> ImportPreview` takes only a path.  It reads image
  format and dimensions plus source mtime when possible; otherwise it returns
  the frozen degenerate preview (`date_range` empty, counts zero, `No readable
  image found`) without writes. A decoded format the module does not recognize
  also returns that degenerate result rather than panicking.
- `import_image(path: &Path, journal_root: &Path, import_id: &str,
  progress: Option<&mut dyn FnMut(&ProgressUpdate)>, wire:
  &dyn WireClient) -> Result<ImageImportResult, ImageImportError>` is the
  sole write entry point.  `import_id` is required because the caller always
  supplies it; no defaulting branch is retained.
- `WireClient` has the same one-method shape as depict: `execute(&self,
  request: &GenerateRequest) -> Result<GenerateResponse, ClientError>`.
  `SystemWireClient` resolves `OneShotClient::sibling()`, prefixes `generate`,
  and executes the request.

`ProgressUpdate` is a small public value with `current`, `total`,
`earliest_date`, `latest_date`, and `entities_found`.  The optional mutable
callback is the only progress seam; successful import invokes it once with
`1, 1, day, day, 0`.

## Import result, manifest, and publication

`ImageImportResult` carries the transcript path in `files_created`, the
`CreatedSegment`, the one-element `days_affected` vector derived from the
source mtime, and a description status/error suitable for the caller to
report. It does not derive affected days from output paths.

The created segment is the concrete `solstone_core_import::CreatedSegment`
with its day, segment key, `import.image` stream, and appropriate
`StreamHints`.  This wave writes the generic import manifest through
`solstone_core_import::write_manifest` and returns the segment for a later
wave to feed into `PublicationInput`; it does not publish.  The manifest's
`days_affected` is supplied from the derived one-element day vector, never
parsed back out of output paths.

## Decode, request, and model outcome

Process first validates the boundary conditions that matter: source exists,
is a regular readable file, and decodes using `image`.  Metadata comes from
the decoded image and mtime: local `YYYYMMDD`, `HHMMSS_0`, title from the file
stem, format, dimensions, and local ISO date.

The generate request uses the image decoder's detected format to select the
matching allowed MIME type and base64 encodes the original source bytes.  It
uses the image-import vision prompt and `import.image.vision` context.  It
does not resize or re-encode.  The enabled native formats (GIF, JPEG, PNG, and
WEBP) exactly match the generate wire's frozen image MIME vocabulary.

`interpret_generate` follows depict's complete response boundary:

- generated response yields trimmed description text;
- `Refused(NoEngineConfigured)` is a distinct unavailable outcome, not a hard
  error;
- every other refusal preserves its reason and detail as an unavailable
  description; and
- every `ClientError` variant (`Protocol`, `Decode`, `Io`, and `Resolve`) is
  an unavailable description with its useful detail retained.

Every unavailable outcome happens after original installation. It writes a
transcript containing the reason and returns a result that reports the missing
description; it never silently claims success of the description.

## Transcript and durable writes

The deterministic transcript header is exactly the reference shape: title
heading, blank line, `Type: Image`, optional format, optional dimensions using
U+00D7, optional date, blank line, horizontal rule, and blank line.  It is
followed by a model-derived block consisting of a stable marker line and a
blockquote rendering of either description text or `unavailable — {reason}`.

The renderer accepts a typed description outcome, and exposes/uses a single
model-block rendering helper.  Tests assert the deterministic prefix and the
separate blockquote partition (including the unavailable case), not the
marker's prose.  This makes provenance structural rather than a wording
snapshot.

Install streams the source into a same-directory temporary file, promotes it
with `install_file` using `AtomicWriteOptions { mode: Some(0o600) }`, then
uses `File::set_times(FileTimes::new().set_modified(...))` to restore the
source modification time.  `0o600` is the conservative importer mode, not a
copy of the source mode.  Source bytes, mode, and mtime are never modified.
The transcript is written through journal I/O with the same private file mode.

The error enum has specific variants for missing/non-file or undecodable
source, install failure, manifest failure, and journal-I/O failure. Source
inputs are boundary conditions and receive explicit validation and errors;
wire outcomes are retained as typed unavailable descriptions. Constructed
paths, the typed result, and internally built request are trusted internal
values rather than defensively revalidated.

## Dependencies and policy

`solstone-core-import-sources/Cargo.toml` gains workspace dependencies on
`image`, `base64`, `solstone-core-generate`, and `solstone-core-journal-io`,
plus `tempfile` and only test-specific dependencies needed by the fixture
tests.  Refresh `core/Cargo.lock`.

Add `solstone-core-import-sources` to the journal-I/O wrapper allowlist in
`core/deny.toml`, appending a reason clause that grants import-source segment
writes: the installed private owner original and description transcript under
`chronicle/<day>/import.image/<seg>/`.  This is the honest declared-output
authority; no proxy through `solstone-core-import` is introduced.

Do not add the `image` TIFF feature and do not edit `core/Cargo.toml`.  TIFF
remains advertised and detectable but fails clearly at decode.  This gap is an
out-of-scope follow-up: even decoded TIFF would require conversion because the
generate wire accepts only GIF, JPEG, PNG, and WEBP.

## Tests and stub retirement

Unit tests in `image.rs` cover request construction and response
interpretation with fake `WireClient` implementations, including original-byte
MIME pass-through, no resize/re-encode, no process construction, all response
doors, and the structural transcript partition.

Integration tests in `tests/` cover the durable contract:

- promote `source_immutability.rs` to call real `import_image` with a fake
  wire, asserting an untouched source and copied original bytes, `0o600` mode,
  and preserved mtime;
- one successful import asserts segment contents, returned `CreatedSegment`,
  explicit one-day `days_affected`, and one observable progress update;
- the no-engine and hard-wire doors each assert that the original remains
  installed and the transcript/result report unavailable description;
- fixture-backed preview pins the degenerate `pic.png` preview;
- fixture-backed extension coverage reads the grammar with `include_str!` and
  its pinned `CAPTURE_REV`, then compares the module extension set
  case-insensitively.

Retire `image::reserved_seam` and remove its `MODULE_STUBS` row in `lib.rs`.
Update the stub count from 12 to 11.  Add an implemented-module list to
`tests/stub_table.rs`, following `df22182b6`, and include `image` so seam
reintroduction fails.  `source_immutability.rs` stops relying on the
all-stubs-positive direction its comment identifies as temporary; its real
image import test becomes that promised staging-test promotion while it may
continue checking remaining stub rows separately.

## Open risks

- `NoEngineConfigured` is non-error at the wire boundary but still produces an
  unavailable description result and transcript by deliberate W5 policy.
- TIFF's advertised/detected-but-undecodable status is intentional for this
  wave and must not be mistaken for MIME support.
- Decode reads the source bytes, install streams the source again, and manifest
  creation hashes it a third time. A concurrent source replacement can make
  those artifacts describe different revisions; this limitation is accepted
  for W5.
