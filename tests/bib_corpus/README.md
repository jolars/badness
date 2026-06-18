# BibTeX test corpus

Fixtures driving the bib differential-parse oracle and gauges. Every `*.bib` here is
auto-discovered by:

- `tests/bib_roundtrip.rs` — losslessness (`reconstruct(text) == text`).
- `tests/bib_parse_oracle.rs` — hard entry-recognition floor vs texlab.
- `tests/bib_parse_compat.rs` — soft skeleton-concordance gauge (`task bib-parse-compat`).

Most files are small, hand-written fixtures targeting a specific construct. One is
vendored real-world data:

## Vendored: `biblatex-examples.bib`

The canonical example database shipped with the **biblatex** package — ~92 entries across
15 entry types, exercising real-world constructs (entry sets, `crossref`, brace-protected
casing, multiline values, commands inside fields, `@string` concatenation).

- **Source:** biblatex 3.21, `bibtex/bib/biblatex/biblatex/biblatex-examples.bib`
  (vendored from the Nix store path
  `…-biblatex-3.21-tex/bibtex/bib/biblatex/biblatex/biblatex-examples.bib`; also on CTAN).
- **License:** LPPL 1.3c (the LaTeX Project Public License) — redistribution permitted.
  Authors: Philipp Lehman, Joseph Wright, Audrey Boruvka, Philip Kime, and contributors.
- Kept **byte-for-byte unmodified** so losslessness stays an honest test of real input.
