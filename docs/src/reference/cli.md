# Command-line reference

A formatter, linter, and language server for LaTeX

**Usage:** `badness [OPTIONS] <COMMAND>`

## Options

`--config <PATH>`
:   Path to a `badness.toml` to use instead of discovering one. Applies to `format` and `lint`; ignored by `parse`, `lsp`, and `init`

`--no-config`
:   Ignore any `badness.toml` (project, `$BADNESS_CONFIG`, or global) and use built-in defaults

`--color <WHEN>`
:   When to use color in output

    Default value: `auto`

    Possible values:

    - `auto`: Colorize when writing to a terminal and `NO_COLOR` is unset (default)
    - `always`: Always colorize
    - `never`: Never colorize

`-q`, `--quiet`
:   Suppress non-essential output (errors are still shown). Under `format --check` this drops the per-file diff, leaving the list of files that would be reformatted and the summary

## `badness format`

Format LaTeX source.

With paths, formats each file in place. Reads stdin (to stdout) when given `-`, or when paths are omitted and stdin is not a terminal.

**Usage:** `badness format [OPTIONS] [PATHS]...`

### Arguments

`<PATHS>...`
:   Files or directories to format. Pass `-` for stdin, which is also read when paths are omitted and stdin is not a terminal

### Options

`--check`
:   Report which files would change without writing them. Exits non-zero if any file is not already formatted. Requires path arguments: there is no file on disk to report on when reading stdin

`--stdin-filepath <PATH>`
:   Name the stdin buffer so its language is dispatched by extension (`.bib` → BibTeX, anything else → LaTeX). No file is read or written; only the extension is used. Ignored when paths are given

`--line-width <LINE_WIDTH>`
:   Maximum line width before the formatter breaks a line

`--indent-width <INDENT_WIDTH>`
:   Number of spaces per indent step

`--item-indent <ITEM_INDENT>`
:   How to indent continuation lines in list items

    Possible values:

    - `hang`: Align continuations under the body following a bare `\item ` (default)
    - `indent`: Indent continuations by one indent-width step
    - `none`: Align continuations with the `\item` command

`--wrap <WRAP>`
:   How to lay out line breaks inside a paragraph

    Possible values:

    - `reflow`: Greedy fill: wrap words to the line width (default)
    - `stable`: Preserve acceptable authored breaks and rebalance only nearby text (revision-stable wrapping)
    - `sentence`: One sentence per line (line width ignored)
    - `semantic`: Semantic line breaks (sembr.org): keep authored breaks and add breaks at sentence boundaries
    - `preserve`: Leave authored line breaks untouched

`--math-wrap <MATH_WRAP>`
:   How to lay out line breaks inside display math

    Possible values:

    - `auto`: Derive from the effective wrap mode: preserve → preserve, else break (default)
    - `preserve`: Keep authored line breaks inside display-math bodies
    - `single-line`: Never insert breaks; a long body overflows the line width
    - `break`: Break a too-long body before its top-level operators (amsmath style)

`--line-ending <LINE_ENDING>`
:   How to spell the line breaks in the formatted output

    Possible values:

    - `auto`: Keep the endings the file was written with (default)
    - `lf`: Always LF (`\n`)
    - `crlf`: Always CRLF (`\r\n`)
    - `native`: The platform's convention: CRLF on Windows, LF elsewhere

`--exclude <PATTERN>`
:   Gitignore-style pattern to skip during directory discovery (repeatable). Added on top of any `exclude`/`extend-exclude` from `badness.toml`

`--force-exclude`
:   Apply exclude patterns to files named explicitly on the command line too (they are normally always processed). For runners like pre-commit that pass staged files as arguments

## `badness lint`

Lint LaTeX source, reporting parse diagnostics.

With paths, lints each file. Reads stdin when given `-`, or when paths are omitted and stdin is not a terminal. Exits non-zero if any diagnostics are reported.

**Usage:** `badness lint [OPTIONS] [PATHS]...`

### Arguments

`<PATHS>...`
:   Files or directories to lint. Pass `-` for stdin, which is also read when paths are omitted and stdin is not a terminal

### Options

`--fix`
:   Apply safe autofixes in place, then report what remains. Requires path arguments; has no effect on stdin (there is nothing to write)

`--unsafe-fixes`
:   Also apply fixes that may change typeset output (requires `--fix`)

`--stdin-filepath <PATH>`
:   Name the stdin buffer so its language is dispatched by extension (`.bib` → BibTeX, anything else → LaTeX). No file is read or written; only the extension is used. Ignored when paths are given

`--exclude <PATTERN>`
:   Gitignore-style pattern to skip during directory discovery (repeatable). Added on top of any `exclude`/`extend-exclude` from `badness.toml`

`--force-exclude`
:   Apply exclude patterns to files named explicitly on the command line too (they are normally always processed). For runners like pre-commit that pass staged files as arguments

`--select <RULE>`
:   Run only these rules (repeatable). Overrides `[lint] select` from `badness.toml` when given

`--ignore <RULE>`
:   Disable these rules (repeatable). Overrides `[lint] ignore` from `badness.toml` when given

`--explain <RULE>`
:   Print the description and examples for a rule id, then exit. Ignores paths, config, and fixes

`--output <OUTPUT>`
:   Output format for findings. The human modes write to stderr; `json` writes to stdout

    Default value: `pretty`

    Possible values:

    - `pretty`: Source-snippet output with caret spans, on stderr (default)
    - `concise`: One `path:line:col: severity [rule] message` line per finding, on stderr
    - `json`: A machine-readable JSON array of findings on stdout (`[]` when clean), with byte-offset ranges and fix data

## `badness parse`

Parse LaTeX source and print its concrete syntax tree (CST).

A debugging aid: prints the lossless parse tree as an indented `KIND@range` listing, with token text, followed by any parse errors. With a path, parses that file. Reads stdin when given `-`, or when the path is omitted and stdin is not a terminal.

**Usage:** `badness parse [PATH]`

### Arguments

`<PATH>`
:   File to parse. Pass `-` for stdin, which is also read when the path is omitted and stdin is not a terminal

## `badness lsp`

Run the language server over stdio

**Usage:** `badness lsp`

## `badness inverse-search`

Answer a PDF viewer's inverse (backward) search.

Point your viewer's inverse-search command here — for zathura, `--synctex-editor-command "badness inverse-search --input %{input} --line %{line}"`. The position is handed to a running badness language server, which reveals it in your editor via `window/showDocument`, so the file must belong to a workspace some editor currently has open.

**Usage:** `badness inverse-search [OPTIONS] --input <PATH>`

### Options

`-i`, `--input <PATH>`
:   The `.tex` file the viewer resolved

`-l`, `--line <LINE>`
:   Line number, counting from 1 — what SyncTeX-aware viewers emit.

    Required unless `--line0` is given. Deliberately not enforced by clap, whose message for that would name only `--line` and so send a `--line0` user the wrong way.

`--line0 <LINE>`
:   Line number counting from 0, for a viewer that reports it that way

`--character <COLUMN>`
:   Column, counting from 0, when the viewer supplies one

    Default value: `0`

`--ipc-dir <DIR>`
:   Directory holding the servers' IPC advertisements. Defaults to `$BADNESS_IPC_DIR`, then a per-user directory under the runtime (or temporary) directory

## `badness init`

Write a commented starter `badness.toml` to the current directory

**Usage:** `badness init [OPTIONS]`

### Options

`--force`
:   Overwrite an existing `badness.toml`
