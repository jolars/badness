# Getting Started

badness has three subcommands: `format`, `lint`, and `lsp`. This page walks
through the first two from the command line. For editor integration, see [Editor
Setup](editor-setup.md).

## Formatting a file

Format a file in place:

```sh
badness format paper.tex
```

Pass several paths to format them all:

```sh
badness format intro.tex methods.tex results.tex
```

With no paths, badness reads from standard input and writes the formatted result
to standard output—handy for piping or editor integrations:

```sh
cat paper.tex | badness format
```

## Checking without writing

In CI you usually want to *verify* that files are already formatted rather than
rewrite them. The `--check` flag reports which files would change and exits
non-zero if any are not already formatted:

```sh
badness format --check paper.tex
```

## Linting

`lint` parses each file and reports any diagnostics found, rendered with source
snippets. It exits non-zero when there is at least one diagnostic:

```sh
badness lint paper.tex
```

Like `format`, it reads standard input when given no paths:

```sh
cat paper.tex | badness lint
```

## Adjusting layout

The formatter takes a few style options on the command line:

```sh
badness format --line-width 100 --indent-width 4 --wrap preserve paper.tex
```

See the [CLI Reference](../reference/cli.md) for every flag and the [Wrap
Modes](../reference/wrap-modes.md) page for what `--wrap` controls.
