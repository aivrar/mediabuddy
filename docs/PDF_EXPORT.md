# PDF Export

The PDF-ready manual is:

```text
docs/MEDIA_BUDDY_MANUAL.md
```

It uses normal Markdown headings, tables, fenced code blocks, and simple HTML
page-break markers. This keeps it readable on GitHub and compatible with common
Markdown-to-PDF tools.

## VS Code

1. Open `docs/MEDIA_BUDDY_MANUAL.md`.
2. Use a Markdown PDF extension.
3. Export to PDF.

## Pandoc

If Pandoc is installed:

```powershell
pandoc docs\MEDIA_BUDDY_MANUAL.md -o docs\Media-Buddy-Manual.pdf --toc --pdf-engine=xelatex
```

If a LaTeX engine is not installed, export to HTML first and print to PDF from
the browser:

```powershell
pandoc docs\MEDIA_BUDDY_MANUAL.md -o docs\Media-Buddy-Manual.html --standalone --toc
```

## Screenshot Placement

Add screenshots to:

```text
docs/screenshots/
```

Then replace the screenshot example code blocks in the manual with regular
Markdown image links before exporting:

```markdown
![Search tab](screenshots/search.png)
```
