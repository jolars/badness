# What is Badness?

Badness is a formatter, linter, and language server for LaTeX and BibTeX,
written in Rust. It keeps your source consistently formatted, catches common
mistakes, and helps you navigate your project. You can use it from the command
line, in your editor, and in CI.

Badness supports both document authors and package developers. It works with
papers and bibliographies (`.tex` and `.bib`), as well as package, class, and
documented sources (`.sty`, `.cls`, `.dtx`, and `.ins`). It supports traditional
LaTeX2e code and the LaTeX3 programming layer, expl3.

## Three tools in one

### Formatter

The formatter gives your source a consistent layout, including indentation, line
wrapping, and alignment. Its layout rules produce deterministic output, and
formatting an already formatted file leaves it unchanged. Verbatim content stays
intact.

Use it to format on save, tidy an existing project, or check formatting in CI.
See [Formatting](formatting.md) for examples and style options.

### Linter

The linter reports syntax errors and common mistakes, such as deprecated
commands, unresolved references, and missing bibliography fields. Diagnostics
point to the relevant source, and many rules offer automatic fixes. You can
choose which rules to enable and suppress individual findings with comments.

See [Linting](linting.md) for checks and fixes, or browse the [LaTeX
rules](../reference/linter-rules.md) and [BibTeX
rules](../reference/bib-linter-rules.md).

### Language server

The language server brings formatting and linting into your editor alongside
completions, hover information, document outlines, navigation, and code actions.
It uses the Language Server Protocol, so the same server works across editors.

See [Editor setup](editor-setup.md) to connect your editor and [Language
server](language-server.md) for its capabilities.

## Built around your source

Badness builds a lossless syntax tree that retains every byte of your input,
including whitespace and comments. Its parser recovers from incomplete or
malformed input so that editor features can keep working while you type.
Incremental parsing and cached analysis let it reuse work after edits. See
[Performance](performance.md) for measured speed and memory use, and
[Architecture](../development/architecture.md) for the design.

Formatting handles layout; lint fixes handle content changes, such as replacing
a deprecated command. Project settings in [`badness.toml`](configuration.md)
keep formatting and linting consistent between your editor, the command line,
and CI.

Keep using your TeX engine and build tools to produce PDFs. Badness analyzes
source without running TeX or expanding arbitrary macros.

## Try it

Use the [playground](/playground/) to try formatting in your browser, or follow
[Getting started](getting-started.md) to format and lint your first file.
