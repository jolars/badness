# Formatting

`badness format` lays out LaTeX source deterministically. Output is decided
solely by the formatter's rules and its layout engine---there are no
per-construct special cases to memorize.

## In Place, `stdin`, or check

```sh
badness format paper.tex          # rewrite the file in place
cat paper.tex | badness format    # stdin → stdout
badness format --check paper.tex  # diff, don't write; non-zero if unformatted
```

`--check` prints a diff of the pending change for each file, then a summary; add
`--quiet` to reduce that to the file list and the summary. See [Checking without
writing](getting-started.md#checking-without-writing).

## Style Options

The style flags---`--line-width`, `--indent-width`, and `--wrap`---mirror the
`[format]` section of `badness.toml` and override it for a single run. Each
option's default and meaning is listed in the [Configuration
reference](../reference/configuration.md#format).

For persistent settings, badness reads a `badness.toml` discovered from the
working directory upward; pass `--config <PATH>` to point at a specific file or
`--no-config` to ignore any discovered one. Run `badness init` to write a
starter `badness.toml`.

## Turning the formatter off

Sometimes a block is laid out by hand and should stay that way---a `tikzpicture`
aligned by eye, a table whose columns line up in the source. Comment directives
turn the formatter off over exactly as much as you point at, and content inside
is reproduced byte for byte.

Skip the next construct:

```tex
% badness-format skip: hand-aligned by eye
\begin{tikzpicture}
  \foreach \p/\pos in {A/left, B/left, C/right, D/right}%
  \node[\pos] at (\p) {$\p$};%
\end{tikzpicture}
```

Skip a region:

```tex
% badness-format off
\begin{tabular}{ll}
  a   &   b \\
  ccc &   d \\
\end{tabular}
% badness-format on
```

Skip a whole file, wherever in it the directive sits:

```tex
% badness-format skip-file: generated, do not edit
```

An `off` with no matching `on` runs to the end of the file. The `: <reason>` is
optional everywhere and is never interpreted---it is there for the next person
to read.

Each directive has a bare counterpart that turns off **both** the formatter and
every lint rule over the same span: `% badness skip`, `% badness off` /
`% badness on`, and `% badness skip-file`. Use the `-format` spelling when you
want the linter to keep reporting.

To exclude whole files by path instead, use `exclude`/`extend-exclude` in
`badness.toml`; see the [Configuration
reference](../reference/configuration.md). That is the better tool when you
control the config, since it keeps the directive out of the document.

Two notes on where directives are read. A directive must be its own `%`
comment---in a `.dtx` documentation line the leading `%` is a documentation
margin rather than a comment, so a directive written there is inert (inside a
`macrocode` chunk it works normally). And `% badness-ignore <rule>` is a
different, linter-only family that has nothing to do with layout; see
[Linting](linting.md).

## Guarantees

The formatter is built around a small set of invariants that double as test
oracles:

- **Idempotence**: `format(format(x)) == format(x)`.
- **Losslessness**: the parsed tree reconstructs the input byte-for-byte, so the
  formatter never loses or corrupts content.
- **Protected regions**: verbatim-like content (`verbatim`, `lstlisting`,
  `\verb`, comments) is never altered.
- **Whitespace-only**: formatting changes whitespace, line breaks, and comment
  placement, and nothing else. It never inserts, deletes, or rewrites a token of
  real content.

Content normalizations---rewriting `x^{2}` to `x^2`, or `$$…$$` to `\[…\]`---are
therefore *lint fixes*, not formatting. Run `badness lint --fix` for those.
