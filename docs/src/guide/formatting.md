# Formatting

`badness format` lays out LaTeX source deterministically. The formatter's rules
and layout engine determine the output, so there are no special cases for
individual constructs to memorize.

## In Place, `stdin`, or check

```sh
badness format paper.tex          # rewrite the file in place
cat paper.tex | badness format    # stdin → stdout
badness format --check paper.tex  # diff, don't write; non-zero if unformatted
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

Badness also reads piped input when you omit paths, so the shorter
`cat paper.tex | badness format` works too. At an interactive prompt,
`badness format` with no paths reports a usage error. See [Checking Without
Writing](#checking-without-writing) for `--check` reporting.

## Checking Without Writing

In CI you usually want to *verify* that files are already formatted rather than
rewrite them. The `--check` flag prints a diff of what would change and exits
with a nonzero status if any file needs formatting:

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

The report shows what would change and appears by default. Pass `--quiet` for
just the file list and summary—useful when a first run over an unformatted
project would otherwise flood a CI log:

```sh
badness format --check --quiet .
```

The report goes to stdout and uses color when writing to a terminal. Only errors
use stderr. Override the color setting with `--color always|never`; Badness also
honors `NO_COLOR`.

## Style Options

Style flags such as `--line-width`, `--indent-width`, `--item-indent`, and
`--wrap` mirror the `[format]` section of `badness.toml` and override it for a
single run. The [Configuration reference](../reference/configuration.md#format)
lists each option's default and meaning.

For example, set the line width, indentation, and wrapping mode for one run:

```sh
badness format --line-width 100 --indent-width 4 --wrap preserve paper.tex
```

See the [CLI Reference](../reference/cli.md) for every flag and the
[Configuration reference](../reference/configuration.md#wrap) for what `--wrap`
controls.

For persistent settings, Badness discovers `badness.toml` from each input file's
directory upward; pass `--config <PATH>` to point at a specific file or
`--no-config` to ignore any discovered one. Run `badness init` to write a
starter `badness.toml`. See [Configuration](configuration.md) to share settings
and exclude files from formatting and linting.

Under reflow, textual optional arguments wrap at existing top-level comma-space
boundaries and fill each line to the configured width. Continuation lines are
indented, and brackets remain attached wherever adding a space would change the
argument. Known key-value arguments instead expand to one entry per line when
they do not fit. Opaque braced environment arguments keep values such as
`{section in head/foot}` together instead of wrapping their words. Existing
comments retain their binding; the formatter does not insert `%` markers to
create new break opportunities.

For a literal top-level `\documentclass{cas-sc}` or `\documentclass{cas-dc}`,
Badness treats the braced affiliation fields and the trailing address options of
`\affiliation` as key-value lists. Short lists stay inline; longer lists expand
to one entry per line. Other classes retain ordinary argument formatting, and
definitions in the document or a loaded local package override the class
signature.

In `align` and other math alignment grids, including nested `aligned`
environments, a leading `\label{key}` sits on its own line. Formula rows align
without counting the label toward column widths. Trailing comments stay attached
to their labels, and labels within formula rows stay in place. This layout
applies independently of `math-wrap`.

Under `math-wrap = "break"` (the default unless `wrap = "preserve"`), multiline
sibling environments in display math form separate blocks, even when a joined
line would fit. Intervening expressions sit on their own lines, and punctuation
stays attached to the preceding block. A prefix such as `A =` can still
introduce the first environment on the same line. Explicit `preserve` and
`single-line` math modes retain their existing line-break policies.

Inside `algorithm` and `algorithm2e` environments (including their starred
forms), Badness normalizes text in `\KwIn`, `\KwOut`, `\KwData`, and
`\KwResult`. It indents the braced bodies of `\For`, `\ForEach`, `\ForAll`,
`\While`, `\If`, `\ElseIf`, `\Else`, `\eIf`, and `\Repeat`, placing each
statement ending in `\;` on its own source line. Math spacing commands remain
inside their formulas, and trailing comments stay attached to their statements.
Control-flow commands need their complete braced arguments to receive this
layout; custom commands and forms with parenthesized side comments use the
ordinary fallback. Top-level `\;` statements require a recognized control-flow
call in the environment's direct body, since the separate `algorithm` package
uses the same float name and can use `\;` for ordinary spacing.

## Turning the formatter off

Sometimes you want to preserve a block laid out by hand, such as a `tikzpicture`
aligned by eye or a table whose columns line up in the source. Comment
directives turn the formatter off for the selected construct, region, or file.
Content inside is reproduced byte for byte.

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

An `off` with no matching `on` runs to the end of the file. The optional
`: <reason>` explains the directive to the next reader; Badness does not
interpret it.

Each directive has a bare counterpart that turns off **both** the formatter and
every lint rule over the same span: `% badness skip`, `% badness off` /
`% badness on`, and `% badness skip-file`. Use the `-format` spelling when you
want the linter to keep reporting.

To exclude whole files by path instead, use `exclude`/`extend-exclude` in
`badness.toml`; see the [Configuration
reference](../reference/configuration.md). That is the better tool when you
control the config, since it keeps the directive out of the document.

`% badness-lint` suppresses diagnostics without changing layout and takes an
optional rule name; see [Linting](linting.md).

A directive must be its own `%` comment. On a `.dtx` documentation line, the
leading `%` is a documentation margin rather than a comment, so a directive
written there has no effect. Inside a `macrocode` chunk, it works normally.

## Guarantees

The formatter is built around a small set of invariants that double as test
oracles:

- **Idempotence**: `format(format(x)) == format(x)`.
- **Losslessness**: the parsed tree reconstructs the input byte-for-byte, so the
  formatter never loses or corrupts content.
- **Protected regions**: verbatim-like content (`verbatim`, `lstlisting`,
  `\verb`, comments) is never altered. An environment Badness cannot recognize
  as verbatim, such as one defined through macros the scanner cannot follow, can
  be named in [`[environments]`](../reference/configuration.md#environments),
  which also lets you declare a `\bea` and `\eea` pair defined in a sibling
  `.sty`.
- **Whitespace-only**: formatting changes whitespace, line breaks, and comment
  placement, and nothing else. It never inserts, deletes, or rewrites a token of
  real content.

Content rewrites, such as changing `x^{2}` to `x^2` or `$$…$$` to `\[…\]`, are
therefore *lint fixes*. Run `badness lint --fix` for those.
