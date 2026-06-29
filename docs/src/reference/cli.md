# Command-Line Help for `badness`

This document contains the help content for the `badness` command-line program.

## `badness`

A formatter, linter, and language server for LaTeX

**Usage:** `badness [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `format` — Format LaTeX source
* `lint` — Lint LaTeX source, reporting parse diagnostics
* `parse` — Parse LaTeX source and print its concrete syntax tree (CST)
* `lsp` — Run the language server over stdio
* `init` — Write a commented starter `badness.toml` to the current directory

###### **Options:**

* `--config <PATH>` — Path to a `badness.toml` to use instead of discovering one. Applies to `format` and `lint`; ignored by `parse`, `lsp`, and `init`
* `--no-config` — Ignore any `badness.toml` and use built-in defaults



## `badness format`

Format LaTeX source.

With paths, formats each file in place. With no paths, reads stdin and writes the formatted result to stdout.

**Usage:** `badness format [OPTIONS] [PATHS]...`

###### **Arguments:**

* `<PATHS>` — Files to format. Omit to read from stdin

###### **Options:**

* `--check` — Report which files would change without writing them. Exits non-zero if any file is not already formatted
* `--stdin-filepath <PATH>` — Name the stdin buffer so its language is dispatched by extension (`.bib` → BibTeX, anything else → LaTeX). No file is read or written; only the extension is used. Ignored when paths are given
* `--line-width <LINE_WIDTH>` — Maximum line width before the formatter breaks a line
* `--indent-width <INDENT_WIDTH>` — Number of spaces per indent step
* `--wrap <WRAP>` — How to lay out line breaks inside a paragraph

  Possible values:
  - `reflow`:
    Greedy fill: wrap words to the line width (default)
  - `sentence`:
    One sentence per line. (Not yet implemented — behaves like `preserve`.)
  - `semantic`:
    Semantic line breaks (sembr.org). (Not yet implemented — like `preserve`.)
  - `preserve`:
    Leave authored line breaks untouched

* `--exclude <PATTERN>` — Gitignore-style pattern to skip during directory discovery (repeatable). Added on top of any `exclude`/`extend-exclude` from `badness.toml`



## `badness lint`

Lint LaTeX source, reporting parse diagnostics.

With paths, lints each file. With no paths, reads stdin. Exits non-zero if any diagnostics are reported.

**Usage:** `badness lint [OPTIONS] [PATHS]...`

###### **Arguments:**

* `<PATHS>` — Files to lint. Omit to read from stdin

###### **Options:**

* `--fix` — Apply safe autofixes in place, then report what remains. Requires path arguments; has no effect on stdin (there is nothing to write)
* `--unsafe-fixes` — Also apply fixes that may change typeset output (requires `--fix`)
* `--stdin-filepath <PATH>` — Name the stdin buffer so its language is dispatched by extension (`.bib` → BibTeX, anything else → LaTeX). No file is read or written; only the extension is used. Ignored when paths are given
* `--exclude <PATTERN>` — Gitignore-style pattern to skip during directory discovery (repeatable). Added on top of any `exclude`/`extend-exclude` from `badness.toml`
* `--select <RULE>` — Run only these rules (repeatable). Overrides `[lint] select` from `badness.toml` when given
* `--ignore <RULE>` — Disable these rules (repeatable). Overrides `[lint] ignore` from `badness.toml` when given



## `badness parse`

Parse LaTeX source and print its concrete syntax tree (CST).

A debugging aid: prints the lossless parse tree as an indented `KIND@range` listing, with token text, followed by any parse errors. With a path, parses that file. With no path, reads stdin.

**Usage:** `badness parse [PATH]`

###### **Arguments:**

* `<PATH>` — File to parse. Omit to read from stdin



## `badness lsp`

Run the language server over stdio

**Usage:** `badness lsp`



## `badness init`

Write a commented starter `badness.toml` to the current directory

**Usage:** `badness init [OPTIONS]`

###### **Options:**

* `--force` — Overwrite an existing `badness.toml`



