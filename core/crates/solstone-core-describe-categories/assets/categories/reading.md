{

  "description": "Documents, articles, PDFs, documentation",
  "output": "markdown",
  "extraction": "Extract when viewing a different document, article, or PDF",
  "importance": "high"

}

# Document Reading Text Extraction

Extract text from this document screenshot (PDFs, documentation, articles, ebooks, manuals).

## Header

`# [Document Title or Type]`

## Content Focus

Extract all visible text, preserving document structure:

- Use `##`, `###` for section headings
- Preserve numbered sections (e.g., "2.3 Configuration")
- Use markdown tables for tabular data
- Use code fences with language tags for code examples

## Quality

- Maintain logical reading flow
- Preserve paragraph breaks
- Skip page headers/footers unless meaningful
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
