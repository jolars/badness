# Formatting

`badness format` lays out LaTeX source deterministically. Output is decided solely
by the formatter's rules and its layout engine—there are no per-construct special
cases to memorize.

## In place, stdin, or check

```sh
badness format paper.tex          # rewrite the file in place
cat paper.tex | badness format    # stdin → stdout
badness format --check paper.tex  # report, don't write; non-zero if unformatted
```

## Style options

| Flag | Default | Meaning |
|------|---------|---------|
| `--line-width <N>` | `80` | Maximum line width before the formatter breaks a line. |
| `--indent-width <N>` | `2` | Spaces per indent step. |
| `--wrap <mode>` | `reflow` | How line breaks inside a paragraph are laid out. See [Wrap Modes](../reference/wrap-modes.md). |

> A configuration file is not yet read; style is set through these flags. This is
> expected to change as badness matures.

## Guarantees

The formatter is built around a small set of invariants that double as test
oracles:

- **Idempotence** — `format(format(x)) == format(x)`.
- **Stability** — formatting does not change the parsed structure of a document.
- **Protected regions** — verbatim-like content (`verbatim`, `lstlisting`,
  `\verb`, comments) is never altered.

If the formatter ever produces output that violates one of these, that is a bug to
report, not behavior to work around.
