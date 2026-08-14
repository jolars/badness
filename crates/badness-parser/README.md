# badness-parser

The parser behind [badness](https://badness.dev/), a formatter, linter, and
language server for LaTeX — extracted so that other tools can embed it.

It is a generic, error-tolerant, hand-written recursive-descent parser in the
rust-analyzer mold: it treats input as TeX surface syntax and always produces a
**lossless** [rowan](https://crates.io/crates/rowan) concrete syntax tree
(`reconstruct(text) == text`, byte for byte), with errors carried alongside the
tree instead of aborting it. It covers LaTeX (`.tex`, `.sty`/`.cls`, `.dtx`,
`.ins`) and, in the `bib` module, BibTeX/BibLaTeX (`.bib`) as a parallel
pipeline on the same architecture.

Semantics live in the `semantic` module as a separate layer over the CST: a
command-signature database (arity, verbatim-ness, sectioning — including a
generated signature tier for thousands of package commands), definition
scanning, expl3 support, labels, and document outlines. Meaning reaches the
grammar only through a small set of curated routing facts; everything else stays
in this layer.

The crate builds for `wasm32-unknown-unknown`.

Entry points: `parser::parse` / `parser::parse_with_flavor`, `bib::parse`,
`semantic::SemanticModel::build`, and the typed wrappers in `ast`.

See the [architecture
documentation](https://badness.dev/development/architecture.html#the-parser) for
the design.
