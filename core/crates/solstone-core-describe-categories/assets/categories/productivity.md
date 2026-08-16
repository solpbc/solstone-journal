{

  "description": "Spreadsheets, slides, document editors, task and issue tracking tools, dashboards, other workplace desktop or web apps and professional tools that are not primarily calendar/scheduling views",
  "output": "markdown",
  "extraction": "Extract when different application or service is shown (e.g. ChatGPT vs Docs vs issue tracker)",
  "importance": "high"

}

# Productivity App Text Extraction

Extract text from this productivity screenshot (spreadsheets, slides, document editors, task managers, issue trackers, dashboards, project management tools).

## Header

`# [App Name - Document/View Title]`

## Content Focus

Extract all visible data with appropriate structure:

- **Tables/Spreadsheets**: Use markdown tables, include headers
- **Tasks/Issues**: Include title, status, assignee, and due date if visible
- **Slides**: Use `##` for slide titles, bullets for content
- **Dashboards/Apps**: Preserve labels, values, statuses, and visible hierarchy

## Quality

- Preserve data relationships and hierarchy
- Include key metadata (dates, statuses, assignees)
- Mark unclear text with `[unclear]`
- Mark cut-off text with `...`

Return ONLY the formatted markdown.
