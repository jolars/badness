# Badness <picture><source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/jolars/badness/main/branding/logo-dark.svg"><img src="https://raw.githubusercontent.com/jolars/badness/main/branding/logo.svg" align="right" width="120" alt="" /></picture>

[![Build and
Test](https://github.com/jolars/badness/actions/workflows/build-and-test.yml/badge.svg?branch=main)](https://github.com/jolars/badness/actions/workflows/build-and-test.yml)
[![Documentation](https://github.com/jolars/badness/actions/workflows/docs.yml/badge.svg?branch=main)](https://badness.dev/)
[![Open
VSX](https://img.shields.io/open-vsx/v/jolars/badness?logo=vsix)](https://open-vsx.org/extension/jolars/badness)
[![VS
Code](https://vsmarketplacebadges.dev/version-short/jolars.badness.svg?logo=vsix)](https://marketplace.visualstudio.com/items?itemName=jolars.badness)

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

Badness is available from several sources:

- **crates.io**: `cargo install badness`
- **npm**: `npm install -g badness` (bundles a prebuilt binary)
- **PyPI**: `uv tool install badness`/`pipx install badness`
- **Prebuilt binaries**: from the [releases
  page](https://github.com/jolars/badness/releases)
- **VS Code/Open VSX**: the [**Badness**
  extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
  (also works in Positron and Cursor)
- **From source**: `cargo install --path .` in a checkout

The VS Code/Open VSX extension bundles the `badness` binary and starts the
language server automatically when you open a `.tex` file.

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

Formatting is configurable via a TOML file named
`badness.toml`. See the documentation for the full reference.

The language server runs over stdio (`badness lsp`); see the [editor setup
guide](https://badness.dev/guide/editor-setup.html) for Neovim and VS Code
wiring.

## Documentation

Full documentation lives at **<https://badness.dev/>** (built with
mdBook from [`docs/`](docs/)).

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

[MIT](LICENSE)
