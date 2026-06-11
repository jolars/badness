# CLI Reference

> This page is maintained by hand for now. Generated CLI docs (via
> `clap-markdown`) are planned, at which point this page becomes the rendered
> output.

```text
badness <COMMAND>
```

## `badness format`

Format LaTeX source. With paths, formats each file in place. With no paths, reads
standard input and writes the formatted result to standard output.

```text
badness format [OPTIONS] [PATHS]...
```

| Argument / option | Default | Description |
|-------------------|---------|-------------|
| `[PATHS]...` | — | Files to format. Omit to read from stdin. |
| `--check` | off | Report which files would change without writing them. Exits non-zero if any file is not already formatted. |
| `--line-width <N>` | `80` | Maximum line width before the formatter breaks a line. |
| `--indent-width <N>` | `2` | Number of spaces per indent step. |
| `--wrap <MODE>` | `reflow` | How to lay out line breaks inside a paragraph. One of `reflow`, `sentence`, `semantic`, `preserve`. See [Wrap Modes](wrap-modes.md). |

## `badness lint`

Lint LaTeX source, reporting parse diagnostics. With paths, lints each file. With
no paths, reads standard input. Exits non-zero if any diagnostics are reported.

```text
badness lint [PATHS]...
```

| Argument | Description |
|----------|-------------|
| `[PATHS]...` | Files to lint. Omit to read from stdin. |

## `badness lsp`

Run the language server over stdio. See [Editor Setup](../guide/editor-setup.md).

```text
badness lsp
```

## Global flags

| Flag | Description |
|------|-------------|
| `--version` | Print the version (`badness {{ badness-version }}`). |
| `--help` | Print help. |
