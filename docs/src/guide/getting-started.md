# Getting Started

Badness's main subcommands are `format`, `lint`, and `lsp` (with `parse` and
`init` as helpers). This page walks through formatting and linting from the
command line. For editor integration, see [Editor Setup](editor-setup.md).

## Formatting a File

Format a file in place:

```sh
badness format paper.tex
```

Pass several paths to format them all:

```sh
badness format intro.tex methods.tex results.tex
```

Pass `-` to read from standard input and write the formatted result to standard
output—handy for piping or editor integrations:

```sh
cat paper.tex | badness format -
```

A piped standard input is also read when you pass no paths at all, so the
shorter `cat paper.tex | badness format` works too. At an interactive prompt,
though, where there is nothing to pipe, `badness format` with no paths reports a
usage error rather than silently waiting on the terminal.

## Checking Without Writing

In CI you usually want to *verify* that files are already formatted rather than
rewrite them. The `--check` flag prints a diff of what would change and exits
non-zero if any file is not already formatted:

```sh
badness format --check paper.tex
```

```diff
Diff in paper.tex:12:
 \section{Introduction}
-Some    text with   odd spacing.
+Some text with odd spacing.
1 of 1 file(s) would be reformatted
```

Since `--check` writes nothing, that report is the only account of what would
change, which is why it is shown by default. Pass `--quiet` for just the file
list and the summary---useful when a first run over an unformatted project would
otherwise flood a CI log:

```sh
badness format --check --quiet .
```

The report goes to stdout (only errors use stderr) and is colorized when writing
to a terminal; `--color always|never` overrides that, and `NO_COLOR` is honored.

## Linting

`lint` parses each file and reports any diagnostics found, rendered with source
snippets. It exits non-zero when there is at least one diagnostic:

```sh
badness lint paper.tex
```

Like `format`, it reads standard input when given `-` (or when piped with no
paths):

```sh
cat paper.tex | badness lint -
```

The snippets go to stderr and are colorized when it is a terminal, under the
same `--color always|never` and `NO_COLOR` rules as the `--check` diff. The
`--output concise` and `--output json` forms are meant for other programs to
read, so they stay plain whatever `--color` says.

## Adjusting Layout

The formatter takes a few style options on the command line:

```sh
badness format --line-width 100 --indent-width 4 --wrap preserve paper.tex
```

See the [CLI Reference](../reference/cli.md) for every flag and the
[Configuration reference](../reference/configuration.md#wrap) for what `--wrap`
controls.
