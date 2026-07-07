# Badness <img src='https://raw.githubusercontent.com/jolars/badness/main/branding/logo.svg' align="right" width="120" />

[![Build and
Test](https://github.com/jolars/badness/actions/workflows/build-and-test.yml/badge.svg?branch=main)](https://github.com/jolars/badness/actions/workflows/build-and-test.yml)
[![Lint](https://github.com/jolars/badness/actions/workflows/lint.yml/badge.svg?branch=main)](https://github.com/jolars/badness/actions/workflows/lint.yml)
[![Documentation](https://github.com/jolars/badness/actions/workflows/docs.yml/badge.svg?branch=main)](https://badness.dev/)
[![Open
VSX](https://img.shields.io/open-vsx/v/jolars/badness?logo=vsix)](https://open-vsx.org/extension/jolars/badness)
[![VS
Code](https://vsmarketplacebadges.dev/version-short/jolars.badness.svg?logo=vsix)](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Badness** is a language server, formatter, and linter for LaTeX, built on a
lossless concrete syntax tree.

It parses LaTeX once and serves three tools from that tree:

- **Formatter** (`badness format`): deterministic, rule-based layout.
- **Linter** (`badness lint`): diagnostics with source snippets.
- **Language server** (`badness lsp`): both, live in your editor.

The architecture follows [rust-analyzer](https://rust-analyzer.github.io/): a
generic, error-tolerant, hand-written parser produces a lossless tree, semantics
are layered on top as a separate concern, and recomputation is incremental.
badness never *requires* resolving macros or catcodes to succeed—anything it
cannot statically recognize degrades to generic nodes rather than a crash. Two
properties hold by construction and are enforced as tests: **losslessness** (the
tree reconstructs the input byte-for-byte) and **idempotence** (formatting an
already formatted file changes nothing).

## Installation

Badness is written in Rust. Build from a checkout:

```sh
git clone https://github.com/jolars/badness
cd badness
cargo install --path .
```

### VS Code extension

If you use VS Code or a compatible editor (such as Positron or Cursor), install
the [Badness
extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
from the VS Code Marketplace or the [Open VSX
extension](https://open-vsx.org/extension/jolars/badness). It bundles the
`badness` binary and starts the language server automatically when you open a
`.tex` file.

## Usage

```sh
# Format a file in place (or stdin → stdout with no path)
badness format paper.tex

# Verify formatting without writing—exits non-zero if anything would change
badness format --check paper.tex

# Lint, reporting parse diagnostics
badness lint paper.tex

# Run the language server over stdio
badness lsp
```

Formatter style is set through flags: `--line-width` (default `80`),
`--indent-width` (default `2`), and `--wrap` (`reflow` by default; also
`preserve`, with `sentence`/`semantic` planned). See the documentation for the
full reference.

The language server runs over stdio (`badness lsp`); see the [editor setup
guide](https://badness.dev/guide/editor-setup.html) for Neovim and VS Code
wiring.

## Documentation

Full documentation lives at **<https://badness.dev/>** (built with
mdBook from [`docs/`](docs/)).

## Contributing

Architecture, tenets, and conventions are documented in
[`AGENTS.md`](AGENTS.md), written for both human and AI contributors. In short:
keep the syntactic layer free of semantic knowledge, every parser feature needs
corpus and snapshot tests plus a losslessness assertion, and code stays
`rustfmt`-clean with `clippy` warnings treated as errors.

## License

[MIT](LICENSE)
