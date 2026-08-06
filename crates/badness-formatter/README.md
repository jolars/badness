# badness-formatter

The formatting engine behind [badness](https://badness.dev/), a formatter,
linter, and language server for LaTeX — extracted so that other tools (for
example a dprint Wasm plugin) can embed it.

Formatting is deterministic and rule-based on the lossless CST from
[`badness-parser`](https://crates.io/crates/badness-parser), through a
Wadler/Prettier-style layout engine. The formatter changes only *trivia*
(whitespace, newlines, comments, `.dtx` margins) — it never inserts, deletes, or
rewrites a non-trivia token — and it is idempotent: `fmt(fmt(x)) == fmt(x)`.
Protected regions (`verbatim`, `lstlisting`, `\verb`, comments) are never
altered. Both LaTeX (`.tex`, `.sty`/`.cls`, `.dtx`, `.ins`) and BibTeX (`.bib`,
via the `bib` module) are covered.

The crate builds for `wasm32-unknown-unknown`; the filesystem-facing batch APIs
live in the `badness` CLI crate instead.

Entry points: `formatter::format` / `formatter::format_with_style` with
`FormatStyle`, and `bib::format` / `bib::format_with_style`.

See the [development
documentation](https://badness.dev/development/formatter.html) for the engine's
design.
