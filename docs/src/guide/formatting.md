# Formatting

`badness format` lays out LaTeX source deterministically. Output is decided
solely by the formatter's rules and its layout engine---there are no
per-construct special cases to memorize.

## In Place, `stdin`, or check

```sh
badness format paper.tex          # rewrite the file in place
cat paper.tex | badness format    # stdin → stdout
badness format --check paper.tex  # report, don't write; non-zero if unformatted
```

## Style Options

The style flags---`--line-width`, `--indent-width`, and `--wrap`---mirror the
`[format]` section of `badness.toml` and override it for a single run. Each
option's default and meaning is listed in the [Configuration
reference](../reference/configuration.md#format).

For persistent settings, badness reads a `badness.toml` discovered from the
working directory upward; pass `--config <PATH>` to point at a specific file or
`--no-config` to ignore any discovered one. Run `badness init` to write a
starter `badness.toml`.

## Guarantees

The formatter is built around a small set of invariants that double as test
oracles:

- **Idempotence**: `format(format(x)) == format(x)`.
- **Losslessness**: the parsed tree reconstructs the input byte-for-byte, so the
  formatter never loses or corrupts content.
- **Protected regions**: verbatim-like content (`verbatim`, `lstlisting`,
  `\verb`, comments) is never altered.

Note that formatting *may* normalize structure on purpose (for example, `x^{2}`
becomes `x^2`); it preserves meaning, not the exact parse tree.
